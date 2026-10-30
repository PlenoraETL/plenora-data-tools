# Arrow Geometry Semantics 1.0

Il componente adotta `plenora-arrow-geometry-semantics-v1` al pin del manifesto,
con 11 regole conformi e 14 deviazioni. Il censimento riguarda le operazioni
pubbliche del profilo su Rust, CLI e SDK Python; non certifica i singoli
kernel chiamati fuori dal runner ne catene fra componenti.

## Verifica

Esito locale: 103 scenari conformi e 69 con deviazione dichiarata, ciascuno
verificato su API e CLI (172 scenari, 344 invocazioni).

`cargo test -p plenora-cli --test geometry_semantics --locked` legge le 86 copie
byte per byte di `vectors/arrow-data-v1`, controlla pin, inventario e SHA-256,
e prova piano identita e riproiezione verso EPSG:3857 via API pubblica e CLI.
Le risposte sono confrontate con le attese del contratto, le eccezioni del
profilo DT-ARROW-003 e le deviazioni nominate nel runner. Le deviazioni non
contano come conformita. Nel passaggio i valori e i metadati ricevuti restano
identici; i rifiuti verificano categoria, fase, effetto, retry, exit code e
assenza del file di uscita. Il runner non importa un wheel installato: la
proiezione delle eccezioni Python resta coperta dalla suite SDK.

La riproiezione e una sonda di calcolo con requisiti espliciti. `geo.centroid`
richiede un CRS proiettato nel catalogo di questo componente: sui vettori
geografici rifiuta con `crs/validate` prima dei valori. I vettori interop che
si attendono da centroid un errore sui valori non sono una prova verde qui;
non si cambia quel requisito per farli passare.

## Censimento

| Regola | Stato | Evidenza o comportamento |
| --- | --- | --- |
| GEO-000 | deviazione | Il passaggio rifiuta con crs i CRS resolved fuori tabella o con definizione, e geography con unsupported. |
| GEO-001 | conforme | Il vocabolario canonico conserva gli assi e completa l assenza a unknown; anche il legacy senza prova conserva unknown (correzione 4.0). |
| GEO-002 | deviazione | Tutti gli ingressi senza prova escono unknown; il calcolo dipendente dagli assi lo rifiuta esplicitamente per la policy safety-critical 4.0. |
| GEO-003 | conforme | Vettori con ordinate asimmetriche e confronto byte per byte al confine API e CLI. |
| GEO-004 | deviazione | Un crs_id con definizione resolved viene risolto dalla definizione e rifiutato anche nel passaggio. |
| GEO-005 | deviazione | Declared_unresolved conserva la definizione senza verificarne grammatica e identificatori di primo livello. |
| GEO-006 | deviazione | Una definizione senza identificatore, dichiarata resolved, viene rifiutata anche nel passaggio. |
| GEO-007 | deviazione | Le operazioni geo rifiutano ancora EWKB valido con SRID incorporato; le tabellari lo preservano. |
| GEO-008 | conforme | Flag SRID sotto wkb rifiutato data_mapping/read prima del kernel; il passaggio non decodifica. |
| GEO-009 | conforme | Buona forma prima dello SRID; confronto contro il campo originale; lo SRID assente resta assente sotto EWKB. |
| GEO-010 | conforme | Insiemi di tipi preservati; verifica dei valori contro la lista prima del calcolo. |
| GEO-011 | conforme | Lettore strutturale dei valori prima dei kernel, poi SRID, tipi e dimensioni; limite di 32 livelli. |
| GEO-012 | conforme | Piu geometrie rifiutate unsupported/validate, nessun campo scartato. |
| GEO-013 | deviazione | Le identita nuove partono oltre il massimo osservato, per non riusare identita perse dalla lineage. |
| GEO-014 | deviazione | Il cambio di precisione e visibile nello schema; manca un evento dedicato di normalizzazione nel risultato. |
| GEO-015 | deviazione | Le definizioni resolved vengono rifiutate dal risolutore integrato, anche quando verificabili. |
| GEO-016 | deviazione | Il lettore non applica tutta la grammatica chiusa di crs_id e delle chiavi geometry, e ammette liste types vuote in alcuni stati. |
| GEO-017 | deviazione | Non esiste il confronto delle definizioni nel sottoinsieme conservativo a decimali esatti. |
| GEO-018 | conforme | Nessuna uscita pubblicata sui rifiuti dei vettori; pubblicazione atomica e prove run_artefatti. |
| GEO-019 | deviazione | Non esiste il verificatore completo della grammatica chiusa di definizioni e datum ensemble. |
| GEO-020 | deviazione | Le autorita locali restano unresolved ma il piano puo sostituire il CRS per decisione esplicita. |
| REJ-001 | conforme | Categorie dei rifiuti verificate da API e CLI; WKB e SRID non diventano errori del piano. |
| REJ-002 | deviazione | La lettura dei metadati non ha passate globali separate per vocabolario, CRS e supporto. |
| REJ-003 | conforme | Assi categoria/fase/effetto/retry ed exit 3 verificati sui vettori da memoria e IPC. |
| REJ-004 | conforme | 86 vettori al pin, SHA-256, conformita distinta dalle deviazioni e dall applicabilita del calcolo. |

## Deviazioni e rientro

Ambito comune: operazioni pubbliche Rust, CLI e SDK Python. Il manifesto
v4 espande ogni deviazione sulle tre superfici; non dichiara artefatti runtime.

| Regola | Hazard | Rientro |
| --- | --- | --- |
| GEO-000 | Input conformi non attraversano il componente. | Separare conformita, supporto del calcolo e trasporto dei metadati. |
| GEO-002 | Il calcolo con unknown viene rifiutato anche quando GEO-002 lo permetterebbe. | La policy locale richiede evidenza degli assi: dichiarazione esplicita o provenienza certa, senza scambi impliciti. |
| GEO-004 | Rifiuto di input conforme; nessun calcolo sostitutivo silenzioso. | Leggere identita e definizione separatamente, preservandole nel passaggio. |
| GEO-005 | Una dichiarazione contraddittoria puo attraversare un piano tabellare. | Verificare grammatica e identificatori anche senza risoluzione. |
| GEO-006 | Rifiuto di input conforme. | Trasportare la definizione opaca senza chiamare il risolutore. |
| GEO-007 | Interoperabilita del calcolo ridotta; nessuna perdita silenziosa dello SRID. | Conversione EWKB senza perdita dopo la verifica del campo. |
| GEO-013 | Identificatori diversi dal minimo libero richiesto dal contratto. | Distinguere assegnazione iniziale e identita derivate senza reintrodurre alias. |
| GEO-014 | Il consumatore che non confronta gli schemi non vede la normalizzazione. | Esporre e verificare un avviso di normalizzazione sulle superfici pubbliche. |
| GEO-015 | Calcoli conformi rifiutati; nessun ripiego sulla definizione. | Implementare il verificatore conservativo prima del calcolo. |
| GEO-016 | Metadati non conformi possono attraversare il confine. | Chiudere le grammatiche con prove negative per ogni chiave. |
| GEO-017 | Definizioni verificabili rifiutate, senza approssimazioni numeriche. | Implementare il confronto esatto con oracolo differenziale. |
| GEO-019 | Definizioni verificabili rifiutate; declared_unresolved passa opaco. | Implementare grammatica chiusa e vettori negativi per nodi aggiuntivi. |
| GEO-020 | Una decisione del chiamante puo rendere calcolabile un CRS locale che il contratto vuole indecidibile. | Separare decisione di riparazione e identita locale non confrontabile. |
| REJ-002 | Con piu difetti la prima categoria puo divergere dall ordine normativo. | Validare ciascuna classe sull intero schema prima della successiva. |

Le verifiche aggiunte non risolvono il confronto delle definizioni CRS.
Un rifiuto conservativo resta una deviazione quando il contratto impone il
passaggio o accetta il calcolo; non viene contato come conforme.

Migrazione 4.0 e censimento completo: [errori e limiti](limiti.md#ordine-delle-ordinate).
