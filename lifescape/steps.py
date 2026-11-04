"""Etapes de marche : waypoints compresses <-> tuiles une a une.

Un seul endroit pour eviter les 3 copies (route, nav, movement).
Les waypoints sont des changements de direction ; les steps sont des
tuiles adjacentes, diagonales comprises.
"""


def expand_waypoints(waypoints, start):
    """Tous les pas entre start et la fin des waypoints, sans plafond."""
    steps = []
    here = (start[0], start[1]) if len(start) == 2 else start
    for waypoint in waypoints:
        _level, wx, wz = waypoint
        while here != (wx, wz):
            dx = (wx > here[0]) - (wx < here[0])
            dz = (wz > here[1]) - (wz < here[1])
            here = (here[0] + dx, here[1] + dz)
            steps.append(here)
    return steps


def chunk(steps, cap=24):
    """Decoupe en paquets envoyables (paquet MOVE = 24 pas max)."""
    return [steps[offset:offset + cap] for offset in range(0, len(steps), cap)]
