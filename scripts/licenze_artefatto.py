"""Confeziona e verifica gli avvisi negli ZIP CLI, wheel e sorgenti tar.gz.

I file distribuiti portano l'unione dei quattro grafi, identificata da CRATE.json.
La verifica confronta anche il registro del checkout, non si fida del manifesto
nell'archivio. Il wheel viene riscritto con RECORD aggiornato, prima di firmarlo.
"""
from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
from pathlib import Path
import tarfile
import zipfile

import avvisi_crate as avvisi


def bundle() -> dict[str, bytes]:
    data = avvisi.registro()
    return {'CRATE.json': avvisi.json_bytes(data), **avvisi.contenuti(data)}


def leggi(path: Path) -> dict[str, bytes]:
    """Legge senza estrarre; rifiuta duplicati e percorsi ambigui."""
    output = {}

    def add(name: str, content: bytes) -> None:
        avvisi.percorso(name)
        if name in output:
            raise ValueError('membro duplicato nell artefatto')
        output[name] = content

    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            for info in archive.infolist():
                if not info.is_dir():
                    add(info.filename, archive.read(info))
    else:
        with tarfile.open(path, 'r:gz') as archive:
            for info in archive.getmembers():
                if info.isfile():
                    stream = archive.extractfile(info)
                    if stream is None:
                        raise ValueError('membro tar illeggibile')
                    add(info.name, stream.read())
                elif not info.isdir():
                    raise ValueError('membro tar non regolare')
    return output


def prefisso(files: dict[str, bytes], tipo: str) -> str:
    if tipo == 'wheel':
        metadata = [n for n in files if n.endswith('.dist-info/METADATA')]
        if len(metadata) != 1:
            raise ValueError('metadati wheel assenti o ambigui')
        return metadata[0].rsplit('/', 1)[0] + '/licenses/crate-rust/'
    if tipo == 'sorgente':
        roots = {n.split('/')[0] for n in files}
        if len(roots) != 1 or not all('/' in n for n in files):
            raise ValueError('radice sorgente ambigua')
        return next(iter(roots)) + '/LICENSES/crate-rust/'
    if tipo == 'cli':
        return 'LICENSES/crate-rust/'
    raise ValueError('tipo di artefatto sconosciuto')


def record(files: dict[str, bytes], name: str) -> bytes:
    buffer = io.StringIO(newline='')
    writer = csv.writer(buffer, lineterminator='\n')
    for path, content in sorted(files.items()):
        if path == name:
            continue
        digest = base64.urlsafe_b64encode(hashlib.sha256(content).digest()).rstrip(b'=').decode('ascii')
        writer.writerow([path, 'sha256=' + digest, str(len(content))])
    writer.writerow([name, '', ''])
    return buffer.getvalue().encode('utf-8')


def verifica(path: Path, tipo: str) -> None:
    files = leggi(path)
    prefix = prefisso(files, tipo)
    expected = {prefix + n: c for n, c in bundle().items()}
    actual = {n: c.replace(b'\r\n', b'\n') for n, c in files.items() if n.startswith(prefix)}
    if actual != expected:
        raise ValueError('avvisi mancanti, alterati o registro incompleto nell artefatto')
    if tipo == 'wheel':
        name = prefix.split('/licenses/')[0] + '/RECORD'
        if files.get(name) != record(files, name):
            raise ValueError('RECORD del wheel incoerente')
        if not any(n.endswith(('.pyd', '.so')) for n in files):
            raise ValueError('modulo nativo assente dal wheel')
    elif tipo == 'cli':
        binaries = [n for n in files if '/' not in n and n.startswith('plenora-data')]
        if len(binaries) != 1 or not files[binaries[0]]:
            raise ValueError('eseguibile CLI assente o ambiguo')
    elif not any(n.endswith('/Cargo.lock') for n in files):
        raise ValueError('sorgente senza lock')


def confeziona(source: Path, output: Path, tipo: str) -> None:
    licenses = bundle()
    if tipo == 'cli':
        files = {source.name: source.read_bytes()}
    else:
        files = leggi(source)
    prefix = prefisso(files, tipo)
    if any(n.startswith(prefix) for n in files):
        raise ValueError('artefatto gia confezionato')
    files.update({prefix + n: c for n, c in licenses.items()})
    if tipo == 'wheel':
        dist = prefix.split('/licenses/')[0]
        if any(dist + '/' + n in files for n in ('RECORD.jws', 'RECORD.p7s')):
            raise ValueError('non si modifica un wheel firmato')
        name = dist + '/RECORD'
        files[name] = record(files, name)
    # Il temporaneo e' accanto alla destinazione: replace e' atomico e permette
    # di finalizzare un wheel in posto senza troncare l'originale.
    import tempfile
    with tempfile.TemporaryDirectory(dir=output.parent) as temp:
        temporary = Path(temp) / output.name
        if tipo == 'sorgente':
            import gzip
            with temporary.open('wb') as raw:
                with gzip.GzipFile(fileobj=raw, mode='wb', filename='', mtime=0) as compressed:
                    with tarfile.open(fileobj=compressed, mode='w') as archive:
                        # Conserva i modi del sorgente, compresi gli script eseguibili.
                        with tarfile.open(source, 'r:gz') as original:
                            modes = {i.name: i.mode for i in original.getmembers()}
                        for name, content in sorted(files.items()):
                            info = tarfile.TarInfo(name)
                            info.size = len(content)
                            info.mode = modes.get(name, 0o644)
                            archive.addfile(info, io.BytesIO(content))
        else:
            with zipfile.ZipFile(temporary, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
                for name, content in sorted(files.items()):
                    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                    info.create_system = 3
                    info.external_attr = (0o100755 if tipo == 'cli' and name == source.name else 0o100644) << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(info, content)
        verifica(temporary, tipo)
        temporary.replace(output)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tipo', choices=('cli', 'wheel', 'sorgente'), required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    try:
        if args.output:
            # Il confezionamento verifica anche il grafo effettivo offline.
            avvisi.verifica(avvisi.registro())
            confeziona(args.input, args.output, args.tipo)
        else:
            verifica(args.input, args.tipo)
        print('Licenze artefatto verificate')
        return 0
    except (ValueError, OSError, KeyError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(f'Licenze artefatto: {error}')
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
