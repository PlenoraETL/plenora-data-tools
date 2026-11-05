"""Digest indipendenti del corpus CASI, solo hashlib/hmac Python."""
import hashlib,hmac,json
from pathlib import Path
d={}
for kind in ['md5','sha256','fingerprint','hmac']:
 out=[]
 for i in range(6):
  name='ab'[i%2]
  if kind=='md5': b=f'{i}\x1f{name}'.encode()
  else:
   tag={'sha256':'plenora-sha256-v1','fingerprint':'plenora-fingerprint-v1','hmac':'plenora-hmac-sha256-v1'}[kind]
   b=tag.encode()+b'\0'
   def f(t):
    t=t.encode();return len(t).to_bytes(8,'big')+t
   for col,ty,v in [('id','Int64',str(i)),('name','Utf8',name)]: b+=f(col)+f(ty)+b'\1'+f(v)
  out.append(hmac.new(b'chiave-di-test-del-runner',b,hashlib.sha256).hexdigest() if kind=='hmac' else (hashlib.md5(b).hexdigest() if kind=='md5' else hashlib.sha256(b).hexdigest()))
 d[kind]=out
Path(__file__).with_name('hash_attesi.json').write_text(json.dumps(d,indent=2)+'\n')
