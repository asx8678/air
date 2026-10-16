def mean(xs):
    return sum(xs) / len(xs)


def median(xs):
    xs.sort()
    return xs[len(xs) // 2]
