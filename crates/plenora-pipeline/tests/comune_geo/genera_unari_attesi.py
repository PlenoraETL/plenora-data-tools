"""Oracoli esterni per le fixture geo_unari, mai output Plenora.

Rigenerare con geographiclib==2.1, pyproj==3.7.2, shapely==2.1.2.
Le librerie servono solo alla rigenerazione; Cargo legge il JSON versionato.
"""
import json
from pathlib import Path
from geographiclib.geodesic import Geodesic
from pyproj import Transformer
from shapely.geometry import LineString, Polygon, Point
from shapely import frechet_distance, hausdorff_distance

x, y = 500000, 5000000
lines = [LineString([(x,y),(x+100,y+7),(x+200,y)]), LineString([(x+200,y),(x+300,y+50)]), None]
squares = [Polygon([(a,b),(a+s,b),(a+s,b+s),(a,b+s),(a,b)]) if a is not None else None
           for a,b,s in [(x,y,100),(None,0,0),(x+50,y+50,100),(x+500,y,30)]]
other = LineString([(x,y+20),(x+150,y+30)])
target = Point(x+120,y+40)
geo = Geodesic.WGS84
sphere = Geodesic(6378137 * (1 - 1/298.257223563/3), 0)
points = [(9.19,45.46),None,(12.49,41.9)]
out = {}
for op, shapes, f in [
    ('geo.area', squares, lambda g:g.area),
    ('geo.length', lines, lambda g:g.length),
    ('geo.perimeter', squares, lambda g:g.length),
    ('geo.line_locate_point', lines, lambda g:g.project(target,normalized=True)),
    ('geo.distance', squares, lambda g:g.distance(other)),
    ('geo.hausdorff_distance', lines, lambda g:float(hausdorff_distance(g,other))),
    ('geo.frechet_distance', lines, lambda g:float(frechet_distance(g,other))),
]: out[op] = [None if g is None else f(g) for g in shapes]
for op, geodesic, field in [('geo.geodesic_distance',geo,'s12'),('geo.bearing',geo,'azi1'),('geo.haversine_distance',sphere,'s12')]:
    out[op] = [None if p is None else geodesic.Inverse(p[1],p[0],45.07,7.686)[field] for p in points]
out['geo.bearing'] = [None if a is None else a % 360 for a in out['geo.bearing']]
out['geo.geodesic_line_length'] = [sum(geo.Inverse(*a,*b)['s12'] for a,b in [((45,9),(45.5,10)),((45.5,10),(45,11))])]
polygon=geo.Polygon()
for lon,lat in [(9,45),(9.5,45),(9.5,45.5),(9,45.5)]: polygon.AddPoint(lat,lon)
out['geo.geodesic_area'] = [abs(polygon.Compute()[2])]
transform = Transformer.from_crs('EPSG:32632','EPSG:4326',always_xy=True)
out['reproject'] = [None if g is None else [list(transform.transform(a,b)) for a,b in g.coords] for g in lines]
out['wkt'] = [None if g is None else g.wkt.replace('POLYGON ((','POLYGON((').replace(', ', ',') for g in squares]
Path(__file__).with_name('unari_attesi.json').write_text(json.dumps(out,indent=2)+'\n',encoding='utf-8')
