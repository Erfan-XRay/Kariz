# TEMPORARY (phase 10.4): compares the A/B benchmark rounds of the bench job; removed
# before merge. A = v0.6.1, B = this branch; round 0 is thrown away.
import glob, re, statistics, sys

d = sys.argv[1]
tp_re = re.compile(r'(\S+): 256 MiB echoed in [\d.]+s = (\d+) Mbit/s')
udp_re = re.compile(r'^(.+?), (\d+)-byte packets: (\d+) packets/s')


def read(side, rnd):
    tp, udp = {}, {}
    for line in open(f'{d}/{side}-{rnd}.txt', encoding='utf-8', errors='replace'):
        line = line.replace('test throughput ... ', '').replace('test udp_throughput ... ', '').strip()
        m = tp_re.search(line)
        if m:
            tp[m.group(1)] = float(m.group(2))
        m = udp_re.search(line)
        if m:
            udp[f'{m.group(1)} {m.group(2)}B'] = float(m.group(3))
    return tp, udp


for idx, (kind, unit) in enumerate((('throughput', 'Mbit/s'), ('udp', 'packets/s'))):
    rows = {}
    for side in ('A', 'B'):
        for rnd in (1, 2, 3):
            for k, v in read(side, rnd)[idx].items():
                rows.setdefault(k, {}).setdefault(side, []).append(v)
    changes = []
    print(f'== {kind} ({unit}), mean of rounds 1-3, v0.6.1 -> branch')
    for k, v in rows.items():
        if 'A' in v and 'B' in v:
            a, b = statistics.mean(v['A']), statistics.mean(v['B'])
            changes.append(b / a - 1)
            print(f'{k:36} {a:10.0f} {b:10.0f} {(b / a - 1) * 100:+6.1f}%')
    if changes:
        print(f'-- mean {statistics.mean(changes) * 100:+.1f}%  median {statistics.median(changes) * 100:+.1f}%  n={len(changes)}')
