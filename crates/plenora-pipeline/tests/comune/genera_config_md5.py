"""Rigenera digest MD5 indipendenti per il corpus finito di oracolo_config.

Libreria standard Python: hashlib.md5 sui testi delle fixture (Int64,
Float64 piccoli, bool, Utf8, Binary UTF-8), separati da U+001F come nella
scheda table.md5_hash. Nessun risultato di produzione viene letto.
Eseguire dalla radice del repository. Ordinamento JSON stabile.
"""
from pathlib import Path
import hashlib,json,itertools
vs={'id':[str(i) for i in range(6)],'name':['a','b','a','b','a','b'],'value':[str(i*17+.5) for i in range(6)],'flag':['true','false']*3,'date':['2024-01-02']*6,'date2':['2024-01-03']*6,'json':['{"a":1}']*6,'geom':['wkb-a','wkb-b']*3}
d={}
for nullable in (False,True):
 if nullable:vs['name']=['','b','a','','a','b']
 for a,b in itertools.permutations(vs,2):
  for x,y in zip(vs[a],vs[b]):
   v=x+'\x1f'+y;d[v]=hashlib.md5(v.encode()).hexdigest()
Path('crates/plenora-pipeline/tests/comune/config_md5_attesi.json').write_text(json.dumps(d,sort_keys=True,indent=2)+'\n')
