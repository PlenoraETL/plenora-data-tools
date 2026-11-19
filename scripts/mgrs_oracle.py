"""Offline C1 oracle from NGA 0037 tables 1/2/4/5 and figures B1-B3.

Central-meridian UTM uses Simpson integration of the meridional radius,
not the Krueger implementation under test. UPS uses ellipsoidal polar
stereographic equations. Only the Python standard library is required.
"""
import math
from pathlib import Path

A = 6378137.0
F = 1 / 298.257223563
E2 = F * (2 - F)
E = math.sqrt(E2)


def meridian(lat):
    intervals = 32768
    step = math.radians(lat) / intervals

    def radius(i):
        return A * (1 - E2) / (1 - E2 * math.sin(i * step)**2)**1.5

    return step / 3 * (radius(0) + radius(intervals) + math.fsum(
        (4 if i % 2 else 2) * radius(i) for i in range(1, intervals)))


def ups(lon, lat):
    phi = math.radians(abs(lat))
    scale = 2*A*.994/math.sqrt((1+E)**(1+E)*(1-E)**(1-E))
    rho = scale*math.tan(math.radians(90-abs(lat))/2)/((1-E*math.sin(phi))/(1+E*math.sin(phi)))**(E/2)
    sine, cosine = {0: (0, 1), 90: (1, 0), -90: (-1, 0), 180: (0, -1), -180: (0, -1)}.get(
        lon, (math.sin(math.radians(lon)), math.cos(math.radians(lon))))
    return 2000000+rho*sine, 2000000-(1 if lat > 0 else -1)*rho*cosine


def encode(zone, lat, x, y):
    if zone:
        band = 'CDEFGHJKLMNPQRSTUVWX'[min(19, int((lat+80)//8))]
        col = ['ABCDEFGH', 'JKLMNPQR', 'STUVWXYZ'][(zone-1) % 3][int(x)//100000-1]
        row = 'ABCDEFGHJKLMNPQRSTUV'[(int(y)//100000+(5 if zone % 2 == 0 else 0)) % 20]
        prefix = f'{zone:02}{band}'
    else:
        band = ('Y' if x < 2000000 else 'Z') if lat > 0 else ('A' if x < 2000000 else 'B')
        cols, rows, x0, y0 = {
            'A': ('JKLPQRSTUXYZ', 'ABCDEFGHJKLMNPQRSTUVWXYZ', 8, 8),
            'B': ('ABCFGHJKLPQR', 'ABCDEFGHJKLMNPQRSTUVWXYZ', 20, 8),
            'Y': ('RSTUXYZ', 'ABCDEFGHJKLMNP', 13, 13),
            'Z': ('ABCFGHJ', 'ABCDEFGHJKLMNP', 20, 13)}[band]
        col = cols[int(x)//100000-x0]
        row = rows[int(y)//100000-y0]
        prefix = band
    return f'{prefix}{col}{row}{int(x) % 100000:05}{int(y) % 100000:05}'


def inverse_meridian(northing, latitude):
    target = (northing - (10000000 if latitude < 0 else 0)) / .9996
    estimate = latitude
    for _ in range(3):
        radius = A*(1-E2)/(1-E2*math.sin(math.radians(estimate))**2)**1.5
        estimate -= math.degrees((meridian(estimate)-target)/radius)
    return estimate


def inverse_polar(x, y, north):
    dx, dy = x-2000000, (2000000-y)*(1 if north else -1)
    radius = math.hypot(dx, dy)
    if radius == 0:
        return 0, 90 if north else -90
    lo, hi = 79.0, 90.0
    for _ in range(60):
        mid = (lo+hi)/2
        trial = abs(ups(0, mid)[1]-2000000)
        if trial > radius:
            lo = mid
        else:
            hi = mid
    lon = math.degrees(math.atan2(dx, dy))
    return (-180 if lon == 180 else lon), (lo+hi)/2*(1 if north else -1)


def main():
    result = ['# NGA 0037 tables 1/2/4/5, appendix B; scripts/mgrs_oracle.py',
              '# lon,lat,easting,northing,mgrs5,sw_lon,sw_lat,center_lon,center_lat']
    latitudes = [-79, -64, -40, -8, 0, 8, 32, 48, 64, 70]
    northings = {lat: .9996*meridian(lat)+(10000000 if lat < 0 else 0) for lat in latitudes}
    inverse_latitudes = {(lat, offset): inverse_meridian(math.floor(northings[lat])+offset, lat)
                         for lat in latitudes for offset in [0, .5]}
    for zone in range(1, 61):
        lon = 6*zone-183
        for lat in latitudes:
            y = northings[lat]
            sw_lat = inverse_latitudes[lat, 0]
            center_lat = inverse_latitudes[lat, .5]
            phi = math.radians(center_lat)
            prime = A / math.sqrt(1-E2*math.sin(phi)**2)
            # First-order longitude for a 0.5 m displacement from the central
            # meridian; omitted terms move the point by less than 0.000001 m.
            center_lon = lon + math.degrees(.5/(.9996*prime*math.cos(phi)))
            result.append(f'{lon},{lat},500000,{y:.9f},{encode(zone, lat, 500000, y)},{lon},{sw_lat:.14f},{center_lon:.14f},{center_lat:.14f}')
    for lat in [-90, -89, -85, -80.001, 84.001, 85, 89, 90]:
        for lon in [-180, -132.5, -90, -45, 0, 45, 90, 132.5, 180]:
            x, y = ups(lon, lat)
            sw = inverse_polar(math.floor(x), math.floor(y), lat > 0)
            center = inverse_polar(math.floor(x)+.5, math.floor(y)+.5, lat > 0)
            result.append(f'{lon},{lat},{x:.9f},{y:.9f},{encode(0, lat, x, y)},{sw[0]:.14f},{sw[1]:.14f},{center[0]:.14f},{center[1]:.14f}')
    out = Path(__file__).resolve().parents[1] / 'crates/plenora-kernels-geo/tests/fixtures/mgrs-oracle.csv'
    out.parent.mkdir(exist_ok=True)
    out.write_text('\n'.join(result)+'\n', encoding='utf-8', newline='\n')
    print(f'{len(result)-2} independent vectors written')


if __name__ == '__main__':
    main()
