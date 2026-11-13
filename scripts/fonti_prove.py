"""Inventari ricavati dai sorgenti: nessuna dipendenza di compilazione."""

from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def unici(values, label):
    values = list(values)
    if not values or len(values) != len(set(values)):
        raise ValueError(f"{label}: elenco vuoto o duplicato")
    return sorted(values)


def target_fuzz(root=ROOT):
    manifest = tomllib.loads((root / "fuzz/Cargo.toml").read_text("utf-8"))
    names = unici((item["name"] for item in manifest["bin"]), "target fuzz")
    paths = {item["path"] for item in manifest["bin"]}
    actual = {p.relative_to(root / "fuzz").as_posix()
              for p in (root / "fuzz/fuzz_targets").glob("*.rs")}
    if paths != actual:
        raise ValueError("target fuzz: manifest e sorgenti discordanti")
    return names


def sonde_sdk(root=ROOT):
    source = (root / "crates/plenora-data-py/src/sonde.rs").read_text("utf-8")
    return unici(re.findall(r'#\[pyo3\(name = "(_sonda_\w+)"\)\]', source), "sonde SDK")


def crs_integrati(root=ROOT):
    source = (root / "crates/plenora-core/src/crs/epsg_integrati.rs").read_text("utf-8")
    codes = ["EPSG:" + code for code in re.findall(r'Identificativo::Epsg\((\d+)\)', source)]
    codes += ["OGC:CRS84"] * source.count("Identificativo::OgcCrs84")
    return unici(codes, "CRS integrati")
