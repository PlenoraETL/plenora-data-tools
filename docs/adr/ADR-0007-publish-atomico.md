# ADR 7 — Publish atomico per piattaforma e filesystem

- **Stato**: accettato (design)
- **Decisioni collegate**: D22
- **Riferimenti**: `Architetture.md` §6.3

## Contesto

"Publish atomico" non è una garanzia universale: la sequenza tempfile +
rename/persist è atomica solo a certe condizioni (stesso filesystem, rename
atomico supportato, semantica no-clobber coerente), e la durabilità dopo un
crash della macchina è una proprietà ulteriore e più costosa (fsync). Windows
aggiunge share lock e interferenza antivirus; i filesystem di rete hanno
semantica diversa. La garanzia deve essere precisa, non un'assunzione.

## Decisione

### Due profili

- **`AtomicPublish`** (default): nessun output parziale mai visibile.
  Scrittura su tempfile nella **stessa directory/filesystem** della
  destinazione, `persist` no-clobber (rename atomico, mai sovrascrittura di un
  file esistente), pubblicazione solo a grafo completato con successo.
- **`DurableAtomicPublish`**: in aggiunta, `fsync` del file prima del rename e
  `fsync` della directory dopo il rename. Offre **le più forti garanzie di
  durabilità della piattaforma supportata** — non una garanzia universale:
  controller, cache disco e impostazioni di sistema restano fuori dal
  controllo dell'engine.

Il profilo è parte delle `required_capabilities` del `ValidatedGraph`
(ADR 4): un grafo validato con `DurableAtomicPublish` è rifiutato in un
ambiente che non lo supporta.

### Matrice di supporto

| Ambiente | Stato |
|---|---|
| Filesystem locali Linux (ext4, xfs, btrfs) | supportati |
| Filesystem locali Windows (NTFS) | supportati; comportamento documentato per share lock e antivirus (retry breve con backoff sul persist, poi errore) |
| macOS (APFS) | supportato |
| Filesystem di rete (NFS, SMB) | **fuori scope v1**: rifiutati in validazione dell'output |
| Destinazione su filesystem diverso dal temp | rifiutata (viola il requisito same-filesystem) |

### Regole

- Il tempfile vive nella stessa directory della destinazione (requisito
  same-filesystem per costruzione, non verificato a posteriori).
- **Riconoscimento fail-closed del filesystem**: il publish è consentito solo
  quando l'engine identifica **positivamente** un filesystem locale
  supportato; un filesystem non identificabile è trattato come
  `UnsupportedPublishTarget` (NFS, SMB, FUSE e mount ambigui possono non essere
  riconoscibili in modo portabile — in dubbio, rifiutare).
- Il persist avviene una sola volta, dopo il completamento di tutti i nodi e
  la chiusura del writer IPC.
- In caso di fallimento a qualunque punto: il tempfile è eliminato, nessun
  file parziale resta visibile.
- Un solo output per piano (D4): nessuna transazione multi-file nella v1.

### Fallimento di fsync dopo il rename

Nel profilo `DurableAtomicPublish` il file può essere **già visibile** quando
il `fsync` della directory fallisce: l'output è completo e pubblicato, ma la
durabilità non è confermata. L'esito è tipizzato, non un errore generico:

```rust
enum PublishOutcome {
    Published,
    PublishedButDurabilityUnconfirmed,
}
```

Il chiamante (CLI, adapter) decide come presentarlo; la documentazione utente
deve dichiarare esplicitamente questa casistica.

## Conseguenze

- Test obbligatori: persist no-clobber su file esistente (Windows e Linux);
  crash simulato tra scrittura e persist (nessun output visibile); crash
  simulato dopo persist (output completo); retry antivirus simulato su
  Windows; rifiuto di destinazione su filesystem diverso o di rete.
- Il costo di `DurableAtomicPublish` (fsync doppio) è misurato nei benchmark
  di publish; la scelta del profilo è esplicita nel piano, mai silenziosa.
