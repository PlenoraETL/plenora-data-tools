"""Offline USNG oracle: FGDC 011-2001 figures 2/3 and annexes A/B/E.

Reuses the independent Python meridional integral, not production Rust.
GRS80 (EPSG 7019) and Clarke 1866 (EPSG 7008, defined by both axes).
Central meridians are exact; inverse half-metre east offset uses a local
expansion with omitted displacement below 0.001 mm at these latitudes.
"""
import math
from pathlib import Path

import mgrs_oracle as oracle


def main():
    result = ['# FGDC 011-2001 figures 2/3, annexes A/B/E; scripts/usng_oracle.py',
              '# datum,lon,lat,easting,northing,usng5,sw_lon,sw_lat,center_lon,center_lat']
    for datum, a, f in [('NAD83', 6378137.0, 1/298.257222101),
                        ('NAD27', 6378206.4, (6378206.4-6356583.8)/6378206.4)]:
        oracle.A = a
        oracle.E2 = f*(2-f)
        latitudes = [-79, -64, -40, -8, 0, 8, 32, 48, 64, 70]
        northings = {lat: .9996*oracle.meridian(lat)+(10000000 if lat < 0 else 0)
                     for lat in latitudes}
        inverses = {(lat, offset): oracle.inverse_meridian(math.floor(northings[lat])+offset, lat)
                    for lat in latitudes for offset in [0, .5]}
        for zone in range(1, 61):
            lon = 6*zone-183
            for lat in latitudes:
                y = northings[lat]
                compact = oracle.encode(zone, lat, 500000, y)
                text = f'{zone}{compact[2]} {compact[3:5]} {compact[5:10]} {compact[10:]}'
                if datum == 'NAD27':
                    text += ' (NAD 27)'
                sw_lat, center_lat = inverses[lat, 0], inverses[lat, .5]
                phi = math.radians(center_lat)
                prime = a/math.sqrt(1-oracle.E2*math.sin(phi)**2)
                center_lon = lon+math.degrees(.5/(.9996*prime*math.cos(phi)))
                result.append(f'{datum},{lon},{lat},500000,{y:.9f},{text},{lon},{sw_lat:.14f},{center_lon:.14f},{center_lat:.14f}')
    out = Path(__file__).resolve().parents[1]/'crates/plenora-kernels-geo/tests/fixtures/usng-oracle.csv'
    out.write_text('\n'.join(result)+'\n', encoding='utf-8', newline='\n')
    print(f'{len(result)-2} independent USNG vectors written')


if __name__ == '__main__':
    main()
