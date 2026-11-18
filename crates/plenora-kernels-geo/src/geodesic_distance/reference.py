"""C1 independent point/arc reference, GeographicLib Python 2.1 (G2).

Run with that exact package available on PYTHONPATH. No Rust kernel imports.
The heap encloses the *global* minimum using the metric triangle inequality;
the fixture is the final interval, not an unconstrained local minimizer.
"""

import heapq
import json

import geographiclib
from geographiclib.geodesic import Geodesic


def reference(point, start, end):
    """Return a certified numerical bracket in metres on WGS84."""
    geodesic = Geodesic.WGS84
    inverse = geodesic.Inverse(*start, *end)
    line = geodesic.Line(*start, inverse["azi1"])
    heap = []
    upper = float("inf")
    margin = 1e-6
    evaluations = 0

    def add(lo, hi):
        nonlocal upper, evaluations
        middle = (lo + hi) / 2
        position = line.Position(middle)
        distance = geodesic.Inverse(
            *point, position["lat2"], position["lon2"]
        )["s12"]
        evaluations += 1
        upper = min(upper, distance + margin)
        lower = max(0.0, distance - (hi - lo) / 2 - 4 * margin)
        heapq.heappush(heap, (lower, lo, hi))

    add(0.0, inverse["s12"])
    while heap[0][0] < upper - 0.0005:
        _, lo, hi = heapq.heappop(heap)
        mid = (lo + hi) / 2
        add(lo, mid)
        add(mid, hi)
    return dict(
        point_latlon=point,
        start_latlon=start,
        end_latlon=end,
        lower_m=heap[0][0],
        upper_m=upper,
        evaluations=evaluations,
    )


if __name__ == "__main__":
    if geographiclib.__version__ != "2.1":
        raise RuntimeError("GeographicLib Python 2.1 required")
    cases = [
        ((14.1, 35.0), (12.3, 34.5), (13.7, 37.2)),
        ((72.0, 170.0), (70.0, 165.0), (73.0, -175.0)),
    ]
    print(json.dumps([reference(*case) for case in cases], indent=2))
