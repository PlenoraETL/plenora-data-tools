#!/usr/bin/env bash
# Prerequisiti del fuzzing, controllati PRIMA di lanciare qualunque target.
#
# Sta in un file suo perche' lo smoke e la campagna hanno gli stessi tre
# prerequisiti: due copie diverterebbero, e la prima a divergere sarebbe
# quella che qualcuno esegue meno spesso.
#
# Senza questi controlli Docker fallisce da solo, ma dicendo altro: «pull
# access denied for plenora-rust» quando manca l'immagine — che manda a
# cercare credenziali per un'immagine che non e' su nessun registry — oppure
# «exec: no such file» quando il mount e' vuoto. Nessuno dei due dice cosa
# fare.

# Verifica daemon, immagine e binario. Rende 0 se si puo' partire.
preflight() {
    immagine="$1"
    binario="$2"
    fuzzbin_host="$3"

    # 1. Il daemon, e per primo. `docker image inspect` fallisce allo stesso
    #    modo se l'immagine non c'e' e se il daemon non risponde: senza questo
    #    controllo, un Docker Desktop spento veniva riportato come «immagine
    #    mancante», e la diagnosi mandava a ricostruire un'immagine che c'era
    #    gia'. Una causa falsa e' peggio di nessuna causa.
    if ! docker info >/dev/null 2>&1; then
        echo "ERRORE: il daemon Docker non risponde." >&2
        echo "        Non e' un problema di immagine ne' di binario: nessuno" >&2
        echo "        dei due si puo' nemmeno controllare finche' e' cosi'." >&2
        echo "        Su Windows: avvia Docker Desktop e riprova." >&2
        return 1
    fi

    # 2. L'immagine. Ora che il daemon risponde, l'assenza e' davvero assenza.
    if ! docker image inspect "$immagine" >/dev/null 2>&1; then
        echo "ERRORE: l'immagine '$immagine' non esiste in locale." >&2
        echo "        Non e' su un registry: si costruisce, e la ricetta e'" >&2
        echo "        in docs/release.md, sezione «Fuzzing»." >&2
        return 1
    fi

    # 3. Il binario **configurato**, provato dove verra' eseguito.
    #
    #    Non `-f "$fuzzbin_host/cargo-fuzz"` sull'host: quello controlla un
    #    percorso che nessuno esegue. Il comando vero e' `$binario` dentro il
    #    container, e i due possono non coincidere — chi imposta
    #    `FUZZ_CARGO_FUZZ` a un binario dell'immagine non ha bisogno di alcun
    #    mount, e si sarebbe visto rifiutare una configurazione valida; chi lo
    #    imposta a un altro percorso montato si sarebbe visto approvare un
    #    binario diverso da quello poi eseguito.
    #
    #    Il mount e' lo stesso del run vero, cosi' il preflight prova
    #    l'ambiente che verra' usato e non una sua approssimazione. `--version`
    #    e' la forma piu' corta che dimostra sia presenza sia eseguibilita':
    #    un binario compilato per l'architettura sbagliata esiste e non parte.
    if ! diagnosi=$(MSYS_NO_PATHCONV=1 docker run --rm \
        -v "$fuzzbin_host:/fuzzbin:ro" "$immagine" "$binario" --version 2>&1); then
        echo "ERRORE: '$binario' non e' eseguibile dentro il container." >&2
        if [ ! -d "$fuzzbin_host" ]; then
            echo "        La cartella montata non esiste: FUZZBIN_HOST=$fuzzbin_host" >&2
        else
            echo "        Montata da FUZZBIN_HOST=$fuzzbin_host" >&2
        fi
        echo "        Si installa con la ricetta in docs/release.md, sezione" >&2
        echo "        «Fuzzing». Se lo tieni altrove: FUZZBIN_HOST=... , e se" >&2
        echo "        e' dentro l'immagine: FUZZ_CARGO_FUZZ=..." >&2
        echo "        Docker ha detto: $diagnosi" >&2
        return 1
    fi

    return 0
}
