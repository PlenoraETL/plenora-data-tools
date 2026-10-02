"""Le quattro operazioni: documenti, forme degli input e degli output,
equivalenza fra le forme, determinismo."""

from __future__ import annotations

import asyncio
import json
import pathlib
from typing import Any

import pyarrow as pa
import pyarrow.feather as feather
import pyarrow.ipc as ipc
import pyarrow.parquet as pq
import pytest

import plenora_data as pd
from aiuti import (
    CANARINO,
    piano_filtro_e_ordine,
    piano_identita,
    tabella_semplice,
)


def scrivi_stream(tabella: pa.Table, percorso: pathlib.Path) -> None:
    with ipc.new_stream(percorso, tabella.schema) as scrittore:
        scrittore.write_table(tabella)


def test_catalog() -> None:
    documento = pd.catalog()
    assert documento["plan_format"] == "plenora-data-plan-v1"
    assert len(documento["kernels"]) == 146
    assert documento["registry"]["operations"]
    # Stesso documento a ogni chiamata e dalla forma asincrona.
    assert pd.catalog() == documento
    assert asyncio.run(pd.acatalog()) == documento


def test_describe_di_ogni_forma_della_stessa_tabella(tmp_path: pathlib.Path) -> None:
    tabella = tabella_semplice()
    atteso = pd.describe(tabella)
    assert atteso["rows"] == 5
    assert [colonna["name"] for colonna in atteso["columns"]] == ["id", "nome", "importo"]
    file_ipc = tmp_path / "t.arrow"
    # Senza compressione: la lettura IPC del componente non decodifica LZ4
    # né ZSTD dei buffer IPC (default di `write_feather`).
    feather.write_feather(tabella, file_ipc, compression="uncompressed")
    file_stream = tmp_path / "t.arrows"
    scrivi_stream(tabella, file_stream)
    blocchi = tabella.to_batches(max_chunksize=2)
    forme: list[Any] = [
        file_ipc,
        str(file_ipc),
        file_stream,
        pa.Table.from_batches(blocchi),
        tabella.to_batches()[0],
        pa.RecordBatchReader.from_batches(tabella.schema, blocchi),
    ]
    for forma in forme:
        assert pd.describe(forma) == atteso, type(forma)
    assert asyncio.run(pd.adescribe(tabella)) == atteso


def test_describe_di_una_tabella_vuota() -> None:
    vuota = tabella_semplice().slice(0, 0)
    documento = pd.describe(vuota)
    assert documento["rows"] == 0
    stream_vuoto = pa.RecordBatchReader.from_batches(vuota.schema, [])
    assert pd.describe(stream_vuoto) == documento


def test_validate_con_il_piano_in_ogni_forma(tmp_path: pathlib.Path) -> None:
    piano = piano_filtro_e_ordine()
    file_piano = tmp_path / "piano.json"
    file_piano.write_text(json.dumps(piano), encoding="utf-8")
    tabella = tabella_semplice()
    file_tabella = tmp_path / "t.arrow"
    feather.write_feather(tabella, file_tabella, compression="uncompressed")
    atteso = pd.validate(piano, {"t": tabella})
    assert atteso["plan_format"] == "plenora-data-plan-v1"
    assert [uscita["name"] for uscita in atteso["outputs"]] == ["alti", "ordinati"]
    assert pd.validate(json.dumps(piano), {"t": tabella}) == atteso
    assert pd.validate(file_piano, {"t": file_tabella}) == atteso
    assert asyncio.run(pd.avalidate(piano, {"t": tabella})) == atteso


def test_run_in_memoria_rende_le_tabelle_nell_ordine_del_piano() -> None:
    risultato = pd.run(piano_filtro_e_ordine(), {"t": tabella_semplice()})
    assert list(risultato.tables) == ["alti", "ordinati"]
    ordinati = risultato.tables["ordinati"]
    assert isinstance(ordinati, pa.Table)
    assert ordinati.column("id").to_pylist() == [2, 3, 4, 5]
    assert ordinati.column("nome").to_pylist() == ["b", "c", "d", "e"]
    documento = risultato.result
    assert documento["outputs"] == [
        {"name": "alti", "content_type": "application/vnd.apache.arrow.stream", "rows": 4, "columns": 3},
        {"name": "ordinati", "content_type": "application/vnd.apache.arrow.stream", "rows": 4, "columns": 3},
    ]
    assert [passo["op"] for passo in documento["steps"]] == ["table.filter", "table.sort"]
    # Lo schema pubblicato: versione del contratto e identità dei campi.
    assert ordinati.schema.metadata[b"plenora.contract.version"] == b"1"
    assert all(campo.metadata and b"plenora.field_id" in campo.metadata for campo in ordinati.schema)


def test_run_su_file_scrive_cio_che_rende_in_memoria(tmp_path: pathlib.Path) -> None:
    tabella = tabella_semplice()
    in_memoria = pd.run(piano_filtro_e_ordine(), {"t": tabella})
    for estensione, tipo in [
        ("arrow", "application/vnd.apache.arrow.file"),
        ("arrows", "application/vnd.apache.arrow.stream"),
        ("parquet", "application/vnd.apache.parquet"),
    ]:
        uscite = {
            "alti": tmp_path / f"alti.{estensione}",
            "ordinati": tmp_path / f"ordinati.{estensione}",
        }
        su_file = pd.run(piano_filtro_e_ordine(), {"t": tabella}, outputs=uscite)
        assert su_file.tables == {}
        assert [uscita["content_type"] for uscita in su_file.result["outputs"]] == [tipo, tipo]
        assert su_file.result["steps"] == in_memoria.result["steps"]
        for nome, percorso in uscite.items():
            if estensione == "arrow":
                riletta = feather.read_table(percorso)
            elif estensione == "arrows":
                riletta = ipc.open_stream(percorso).read_all()
            else:
                riletta = pq.read_table(percorso)
            attesa = in_memoria.tables[nome]
            assert riletta.column_names == attesa.column_names
            assert riletta.to_pydict() == attesa.to_pydict()
            if estensione != "parquet":
                assert riletta.equals(attesa, check_metadata=True)
        # Una destinazione esistente: `conflict` senza overwrite, nessun
        # file toccato; con overwrite si riscrive.
        with pytest.raises(pd.PlenoraConflictError) as errore:
            pd.run(piano_filtro_e_ordine(), {"t": tabella}, outputs=uscite)
        assert errore.value.remote_effect == "none"
        pd.run(piano_filtro_e_ordine(), {"t": tabella}, outputs=uscite, overwrite=True)


def test_run_da_input_su_file_e_in_memoria_insieme(tmp_path: pathlib.Path) -> None:
    piano = {
        "version": 1,
        "inputs": ["a", "b"],
        "steps": [{"out": "u", "op": "table.concat", "in": ["a", "b"], "config": {}}],
        "outputs": ["u"],
    }
    tabella = tabella_semplice()
    file_a = tmp_path / "a.arrow"
    feather.write_feather(tabella, file_a, compression="uncompressed")
    misto = pd.run(piano, {"a": file_a, "b": tabella})
    memoria = pd.run(piano, {"a": tabella, "b": tabella})
    assert misto.tables["u"].equals(memoria.tables["u"], check_metadata=True)
    assert misto.result == memoria.result


def test_i_nomi_degli_input_e_degli_output_si_verificano_senza_ripeterli(
    tmp_path: pathlib.Path,
) -> None:
    tabella = tabella_semplice()
    casi: list[dict[str, Any]] = [
        {"inputs": {CANARINO: tabella}},
        {"inputs": {}},
        {"inputs": {"t": tabella}, "outputs": {"alti": tmp_path / "a.arrow"}},
        {
            "inputs": {"t": tabella},
            "outputs": {
                "alti": tmp_path / "a.arrow",
                "ordinati": tmp_path / "o.arrow",
                CANARINO: tmp_path / "x.arrow",
            },
        },
    ]
    for argomenti in casi:
        with pytest.raises(pd.PlenoraInvalidConfigurationError) as errore:
            pd.run(piano_filtro_e_ordine(), **argomenti)
        assert CANARINO not in str(errore.value)
        assert errore.value.phase == "prepare"
        assert not list(tmp_path.iterdir()), "nessun file scritto"


def test_identita_e_tipi_di_colonna() -> None:
    tabella = pa.table(
        {
            "testo_lungo": pa.array(["a", None], pa.large_string()),
            "binario_lungo": pa.array([b"x", None], pa.large_binary()),
            "elenco": pa.array([[1, 2], None], pa.list_(pa.int64())),
            "struttura": pa.array([{"a": 1}, None], pa.struct([("a", pa.int64())])),
            "decimale": pa.array([1, None], pa.decimal128(10, 2)),
            "istante": pa.array([0, None], pa.timestamp("us", "Europe/Rome")),
            "dizionario": pa.array(["a", "a"]).dictionary_encode(),
            "vista": pa.array(["a", None], pa.string_view()),
        }
    )
    uscita = pd.run(piano_identita(), {"t": tabella}).tables["t"]
    for campo in tabella.schema:
        attesa = tabella.column(campo.name)
        trovata = uscita.column(campo.name)
        if campo.type == pa.large_string():
            # Il runner porta LargeUtf8 a Utf8 all'ingresso (README della
            # radice, «Metadati Arrow»): stessi valori, tipo Utf8.
            assert trovata.type == pa.string()
            assert trovata.equals(attesa.cast(pa.string()))
        else:
            assert trovata.type == campo.type, campo.name
            assert trovata.equals(attesa), campo.name


def test_tipi_che_il_runner_non_supporta_si_rifiutano() -> None:
    for colonna in [
        pa.RunEndEncodedArray.from_arrays([2], pa.array([1])),
        pa.UnionArray.from_sparse(pa.array([0, 0], pa.int8()), [pa.array([1, 2])]),
    ]:
        with pytest.raises(pd.PlenoraUnsupportedError):
            pd.run(piano_identita(), {"t": pa.table({"c": colonna})})
        with pytest.raises(pd.PlenoraUnsupportedError):
            pd.validate(piano_identita(), {"t": pa.table({"c": colonna})})


def test_determinismo() -> None:
    tabella = tabella_semplice()
    primo = pd.run(piano_filtro_e_ordine(), {"t": tabella})
    for _ in range(3):
        altro = pd.run(piano_filtro_e_ordine(), {"t": tabella})
        assert altro.result == primo.result
        for nome, tabella_uscita in primo.tables.items():
            assert altro.tables[nome].equals(tabella_uscita, check_metadata=True)
    asincrono = asyncio.run(pd.arun(piano_filtro_e_ordine(), {"t": tabella}))
    assert asincrono.result == primo.result
    for nome, tabella_uscita in primo.tables.items():
        assert asincrono.tables[nome].equals(tabella_uscita, check_metadata=True)
    # Gli input non cambiano.
    assert tabella.equals(tabella_semplice(), check_metadata=True)


def test_il_risultato_non_si_modifica() -> None:
    risultato = pd.run(piano_identita(), {"t": tabella_semplice()})
    with pytest.raises(TypeError):
        risultato.tables["altro"] = tabella_semplice()  # type: ignore[index]
    with pytest.raises(AttributeError):
        risultato.result = {}  # type: ignore[misc]
