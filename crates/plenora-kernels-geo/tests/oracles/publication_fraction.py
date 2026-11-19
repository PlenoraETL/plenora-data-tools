"""GOP-015A/020: independent intersections with 7*x + 3*y = 21.

Regenerate the p3b reader corpus; no kernel output is used.
"""
from fractions import Fraction as F
from pathlib import Path

state = 42

def random():
    global state
    state = (state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
    return (state >> 11) / (1 << 53)

def hit(a, b):
    x, y = map(F, a)
    dx, dy = F(b[0]) - x, F(b[1]) - y
    t = (21 - 7*x - 3*y) / (7*dx + 3*dy)
    assert 0 < t < 1
    return float(x+t*dx), float(y+t*dy)

rows = []
for _ in range(200):
    a = (0.2 + random(), 0.2 + random())
    b = (2.0 + 2.0*random(), 5.0 + 3.0*random())
    random()  # reader y0
    c = (min(b[0], 2.5), 2.0)
    face = 7*F(c[0]) + 6 < 21
    h = hit((2., 8.), c) if face else (0., 0.)
    row = (*a, *b, *hit(a,b), c[0], int(face), *hit((0.,6.5),(2.,8.)), *h, int(face and 7*F(h[0])+3*F(h[1]) < 21))
    rows.append(" ".join(map(repr,row)))
Path(__file__).with_name("publication_fraction.txt").write_text("\n".join(rows)+"\n", encoding="utf-8", newline="\n")
print("Fraction: 200 unions,", sum(row.split()[7] == "1" for row in rows), "completion faces")

# Rational parametrization of quarter circles, as in reader oracle.py.
def quarter(center, start, turn, radius, tol):
    n = 1
    while radius > 2*F(tol)*n*n:
        n *= 2
    result = []
    for k in range(n+1):
        t = F(k,n)
        c, s = (1-t*t)/(1+t*t), turn*2*t/(1+t*t)
        x, y = start
        result.append((center[0]+radius*(x*c-y*s), center[1]+radius*(x*s+y*c)))
    return result

hairpin = [(F(0),F(-1))]
hairpin += quarter((F(10),F(0)),(0,-1),1,F(1),0.01)
hairpin += quarter((F(10),F(0.1)),(1,0),1,F(1),0.01)
hairpin += [(F(0),F(0.1)+1)]
r = F(1.1)
crossing = [(F(-5),r)] + quarter((F(0),F(0)),(0,1),-1,r,0.01)
crossing += [(r,-5+r),(3-r,-5+r),(3-r,2-r),(F(-5),2-r)]
for name, points in [("hairpin",hairpin),("crossing",crossing)]:
    Path(__file__).with_name(name+"_fraction.txt").write_text(
        "\n".join(f"{float(x)!r} {float(y)!r}" for x,y in points)+"\n", encoding="utf-8", newline="\n")
    print(name,len(points),"exactly rounded vertices")

# Reader oracle.py: Decimal 200 digits for the irrational parallel shifts.
from decimal import Decimal, localcontext
import math
with localcontext() as ctx:
    ctx.prec = 200
    inputs = [(10*math.cos(i*0.01), 10*math.sin(i*0.01)) for i in range(46)]
    output = []
    for a,b in zip(inputs, inputs[1:]):
        ax,ay,bx,by = map(Decimal, (*a,*b))
        dx,dy = bx-ax,by-ay
        length = (dx*dx+dy*dy).sqrt()
        output.extend([(float(ax+dy/length),float(ay-dx/length)),
                       (float(bx+dy/length),float(by-dx/length))])
    for name,points in [("dense_input",inputs),("dense_offset",output)]:
        Path(__file__).with_name(name+".txt").write_text(
            "\n".join(f"{x!r} {y!r}" for x,y in points)+"\n", encoding="utf-8", newline="\n")
    print("Decimal(200):",len(inputs),"input vertices,",len(output),"offset vertices")
