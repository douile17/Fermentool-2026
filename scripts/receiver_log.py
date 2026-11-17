"""Log a second balance, under the vessel that receives the feed, once a second.

Kept apart from the daemon on purpose: it never touches the pump, the feed
balance or the control thread. Its CSV is then compared to a run's journal
by refill_k_check.py, to see what the feed pump really delivered during a
refill (the feed balance cannot: the refill pours into the same bottle).

    python scripts/receiver_log.py COM9 receiver.csv [--baud 9600]

The balance must speak MT-SICS ("SI" answered by "S S  123.4 g"), as the
feed balance does. Stop with Ctrl+C; the file is flushed at every line.
"""

import argparse
import datetime as dt
import re
import time

import serial

REPLY = re.compile(rb"^S\s+([SD])\s+(-?[\d.]+)\s+g")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("port")
    ap.add_argument("out")
    ap.add_argument("--baud", type=int, default=9600)
    ap.add_argument("--every", type=float, default=1.0, help="seconds between reads")
    a = ap.parse_args()
    port = serial.Serial(a.port, a.baud, timeout=1.5)
    with open(a.out, "a", encoding="utf-8") as f:
        if f.tell() == 0:
            f.write("wall_time,weight_g,stable\n")
        next_t = time.monotonic()
        while True:
            port.reset_input_buffer()
            port.write(b"SI\r\n")
            line = port.readline()
            now = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
            m = REPLY.match(line.strip())
            if m:
                f.write(f"{now},{float(m.group(2))},{int(m.group(1) == b'S')}\n")
                f.flush()
            else:
                print(f"{now} no weight: {line!r}")
            next_t += a.every
            time.sleep(max(0.0, next_t - time.monotonic()))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
