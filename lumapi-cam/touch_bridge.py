import sys
import os
import struct
import json
import socket

# Log helper
def log(msg):
    with open('/tmp/lumapi-media-playback.log', 'a') as f:
        f.write(f"BRIDGE: {msg}\n")
    print(f"BRIDGE: {msg}")

log("Starting touch bridge...")

# Find touchscreen event device
def find_touchscreen():
    for name in os.listdir('/sys/class/input'):
        if name.startswith('event'):
            try:
                with open(f'/sys/class/input/{name}/device/name', 'r') as f:
                    dev_name = f.read().lower()
                if 'touch' in dev_name or 'waveshare' in dev_name or 'goodix' in dev_name or 'ts' in dev_name:
                    log(f"Found touchscreen: {dev_name.strip()} at /dev/input/{name}")
                    return f'/dev/input/{name}'
            except Exception:
                continue
    log("No touchscreen found by name, falling back to /dev/input/event0")
    return '/dev/input/event0'

device_path = find_touchscreen()

# Open touch device
try:
    fd = open(device_path, 'rb')
except Exception as e:
    log(f"Failed to open {device_path}: {e}")
    sys.exit(1)

# Format for input_event
# l = long, H = unsigned short, i = int
# This automatically handles 32-bit and 64-bit timeval
if struct.calcsize('L') == 8:
    EVENT_FORMAT = 'llHHi'
    EVENT_SIZE = 24
else:
    EVENT_FORMAT = 'llHHi'
    EVENT_SIZE = 16

log(f"Using EVENT_SIZE={EVENT_SIZE}")

current_x = -1
current_y = -1

# Connect to mpv IPC socket if available
ipc_path = sys.argv[1] if len(sys.argv) > 1 else '/tmp/mpv-socket'
log(f"Connecting to mpv IPC at {ipc_path}")

def send_mpv_command(cmd):
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(ipc_path)
        payload = json.dumps({"command": cmd}) + "\n"
        s.sendall(payload.encode('utf-8'))
        s.close()
        log(f"Sent command to mpv: {cmd}")
    except Exception as e:
        log(f"Failed to send command to mpv: {e}")

while True:
    try:
        data = fd.read(EVENT_SIZE)
        if not data:
            break
        sec, usec, etype, ecode, evalue = struct.unpack(EVENT_FORMAT, data)
        
        # EV_ABS
        if etype == 3:
            if ecode == 0 or ecode == 53: # ABS_X or ABS_MT_POSITION_X
                current_x = evalue
            elif ecode == 1 or ecode == 54: # ABS_Y or ABS_MT_POSITION_Y
                current_y = evalue
        # EV_KEY
        elif etype == 1:
            if ecode == 330: # BTN_TOUCH
                if evalue != 0: # Touch Down
                    log(f"Touch DOWN at raw X={current_x}, Y={current_y}")
                    
                    # Map raw physical coordinates to visual landscape (640x480)
                    # The user confirmed X mapping was correct, but Y was inverted (top instead of bottom)
                    vx = (480 - current_y) * (640.0 / 480.0)
                    vy = current_x * (480.0 / 640.0)

                    log(f"Mapped visual coords: vx={vx}, vy={vy}")

                    # 1. Exit button (Top Right): vx=600, vy=40 (96x96 icon)
                    if vx >= 540 and vy <= 100:
                        send_mpv_command(["quit"])
                    
                    # 2. Seek Back: vx=220, vy=430 (100x100 icon)
                    elif 150 <= vx <= 290 and 370 <= vy <= 480:
                        send_mpv_command(["seek", -5, "relative"])
                    
                    # 3. Play/Pause: vx=320, vy=430 (100x100 icon)
                    elif 270 <= vx <= 370 and 370 <= vy <= 480:
                        send_mpv_command(["cycle", "pause"])
                    
                    # 4. Seek Forward: vx=420, vy=430 (100x100 icon)
                    elif 350 <= vx <= 490 and 370 <= vy <= 480:
                        send_mpv_command(["seek", 5, "relative"])
    except KeyboardInterrupt:
        break
    except Exception as e:
        log(f"Error in read loop: {e}")
        break

log("Exiting touch bridge.")
