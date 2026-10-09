import sys, numpy as np
def read_pgm(p):
    d = open(p, "rb").read()
    parts, i = [], 0
    while len(parts) < 4:
        while d[i:i+1].isspace(): i += 1
        j = i
        while not d[j:j+1].isspace(): j += 1
        parts.append(d[i:j]); i = j
    i += 1
    w, h, mx = int(parts[1]), int(parts[2]), int(parts[3])
    a = np.frombuffer(d[i:], ">u2" if mx > 255 else "u1").reshape(h, w)
    return a, mx
for p in sys.argv[1:]:
    a, mx = read_pgm(p)
    print(p, "max", mx, "mean %.2f" % a.mean(), "vals", np.unique(a)[:10], "low4 nonzero frac %.3f" % ((a & 15) != 0).mean() if mx > 255 else "")
