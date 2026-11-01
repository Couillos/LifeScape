"""Multi-level route planning through stairs, ladders, trapdoors and portals.

The planner works on a small graph: start, destination and both sides of
every declared transition. Walk edges use straight-line distance as a
cheap estimate; the actual path is computed by ``walk_to`` when the leg
is followed, so planning stays fast even with hundreds of transitions.
"""


class Portal:
    def __init__(self, name, frm, to, ride):
        self.name = name
        self.frm = frm
        self.to = to
        self.ride = ride

    def __repr__(self):
        return "Portal(%s %s -> %s)" % (self.name, self.frm, self.to)


class RoutePlanner:
    def __init__(self, router, collision):
        self.router = router
        self.collision = collision
        self.portals = []

    def add_portal(self, portal):
        self.portals.append(portal)

    def plan(self, start, dest, skip_walks=()):
        nodes = [start, dest]
        for portal in self.portals:
            nodes.append(portal.frm)
            nodes.append(portal.to)
        unique = [
            node for node in dict.fromkeys(nodes)
            if self.collision is None or self.collision.has(node[1], node[2], node[0])
        ]
        if start not in unique or dest not in unique:
            return None
        best = {start: (0, [])}
        done = set()
        while True:
            node = None
            for candidate, (cost, _legs) in best.items():
                if candidate not in done and (node is None or cost < best[node][0]):
                    node = candidate
            if node is None or node == dest:
                break
            done.add(node)
            cost, legs = best[node]
            for other in unique:
                if other in done or other == node or other[0] != node[0]:
                    continue
                distance = abs(other[1] - node[1]) + abs(other[2] - node[2])
                if distance == 0:
                    continue
                # A walk the router already refused: do not propose it again.
                if (node, other) in skip_walks:
                    continue
                # Walk legs are estimates here and validated for real when the
                # route is followed (a failed leg drops its transition and the
                # route is planned again), so estimate every pair: pruning
                # long walks would forbid legitimate treks like the castle to
                # the Wizards' Tower.
                if other not in best or cost + distance < best[other][0]:
                    best[other] = (cost + distance, legs + [("walk", other)])
            for portal in self.portals:
                if portal.frm != node or portal.to in done:
                    continue
                if portal.to not in best or cost + 1 < best[portal.to][0]:
                    best[portal.to] = (cost + 1, legs + [("portal", portal)])
        if dest not in best:
            return None
        return best[dest][1]
