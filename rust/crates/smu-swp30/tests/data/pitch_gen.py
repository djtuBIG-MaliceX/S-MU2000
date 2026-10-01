import math
def iroot(x, n):
    if x < 2: return x
    hi = 1 << ((x.bit_length() + n - 1)//n + 1)
    lo = 0
    while lo < hi:
        mid = (lo + hi + 1)//2
        if mid**n <= x: lo = mid
        else: hi = mid - 1
    return lo
rows = [iroot(2**(12288+i), 1024) for i in range(0x400)]
bad = [i for i in range(1, 0x400) if int(math.pow(2.0, i/1024.0)*4096.0) != rows[i]]
print("mismatch i>=1:", bad)
mm = min((min(math.pow(2.0, i/1024.0)*4096.0 - math.floor(math.pow(2.0, i/1024.0)*4096.0),
              math.ceil(math.pow(2.0, i/1024.0)*4096.0) - math.pow(2.0, i/1024.0)*4096.0), i) for i in range(1, 0x400))
print("min margin i>=1:", mm)
with open("pitch_table.txt","w") as f:
    for j in range(0, 0x400, 16):
        f.write("        " + " ".join(f"{k:#06x}," for k in rows[j:j+16]) + "\n")
print("rows:", len(rows), "first:", rows[:4], "last:", rows[-2:])
