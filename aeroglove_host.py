"""
aeroglove host-side translator.

reads the firmware's uart stream through the st-link virtual com port
and converts it into either a virtual xbox 360 gamepad (racing mode)
or mouse movement (mouse mode).

requirements (install once):
    pip install pyserial vgamepad mouse

vgamepad also requires the ViGEmBus driver on Windows. The first time you run
this script, vgamepad will offer to install it automatically (or download from
https://github.com/nefarius/ViGEmBus/releases).

usage:
    1. plug in the aeroglove (st-link usb cable).
    2. find the com port in device manager (e.g. "USB Serial Device (COM10)").
    3. edit COM_PORT below if needed.
    4. run:  python aeroglove_host.py
    5. tilt the glove to move the joystick, long-press the button to switch modes.
"""

import sys
import time

import serial        # reads data from the serial port (com10)
import vgamepad as vg  # creates a virtual xbox 360 controller through the vigem driver
import mouse         # controls the system mouse cursor


#  configuration 
COM_PORT = "COM10"       # change this if your st-link shows up on a different port
BAUD_RATE = 115_200      # must match what the firmware sends (115200 baud)

# how many degrees of tilt equals full joystick deflection
FULL_DEFLECTION_DEG = 60.0

# how fast the mouse moves - pixels per second per degree of tilt
MOUSE_SENSITIVITY = 12.0

# ignore tilt smaller than this so a still hand doesn't cause drift
DEAD_ZONE_DEG = 3.0


# helpers 
def apply_dead_zone(value, dead_zone):
    # return 0 if the value is within the dead zone, otherwise return it unchanged
    if abs(value) < dead_zone:
        return 0.0
    return value


def main():
    print(f"Opening {COM_PORT} @ {BAUD_RATE} baud ...")
    try:
        ser = serial.Serial(COM_PORT, BAUD_RATE, timeout=0.5)  # open the serial port, 0.5s read timeout
    except serial.SerialException as e:
        print(f"ERROR: could not open {COM_PORT}: {e}")
        print("Tip: check Device Manager for the right COM port and update COM_PORT in this file.")
        sys.exit(1)

    print("Setting up virtual Xbox 360 gamepad ...")
    gamepad = vg.VX360Gamepad()  # create the virtual controller - windows now sees an xbox gamepad

    mouse_pressed_state = False   # tracks whether we're currently holding the virtual left-click
    button_held_state = False     # tracks the physical button state (not currently used directly)
    last_time = time.monotonic()  # timestamp of the last packet, used to compute dt for mouse movement
    last_status_print = 0.0       # timestamp of the last status line printed to terminal

    print("Ready. Tilt the glove to play. Press Ctrl+C to exit.\n")

    try:
        while True:
            raw = ser.readline()  # block until we get a full line (ends with \n) or timeout
            if not raw:
                continue  # timeout with no data, try again

            try:
                line = raw.decode("ascii", errors="ignore").strip()  # decode bytes to string, remove whitespace
            except UnicodeDecodeError:
                continue  # skip any corrupted lines

            parts = line.split()  # split on whitespace: ['R', '12.50', '-3.25', '0']
            if len(parts) != 4:
                continue  # not a valid packet, skip it

            mode = parts[0]  # 'R' for racing, 'M' for mouse
            try:
                pitch = float(parts[1])   # forward/backward tilt in degrees
                roll = float(parts[2])    # left/right tilt in degrees
                button = int(parts[3])    # 0 = released, 1 = held
            except ValueError:
                continue  # couldn't parse the numbers, skip this line

            now = time.monotonic()
            dt = now - last_time   # time since last packet in seconds - needed for smooth mouse movement
            last_time = now

            # remove small jitter around zero so a steady hand doesn't move anything
            pitch_dz = apply_dead_zone(pitch, DEAD_ZONE_DEG)
            roll_dz = apply_dead_zone(roll, DEAD_ZONE_DEG)

            if mode == "R":
                #  Gamepad mode 
                # Convert tilt into Xbox 360 stick coordinates (-32768..32767)
                def to_axis(deg):
                    norm = max(-1.0, min(1.0, deg / FULL_DEFLECTION_DEG))  # clamp tilt to -1..1
                    return int(norm * 32767)  # scale to xbox axis range

                x_axis = to_axis(roll_dz)       # left/right tilt controls the x axis
                y_axis = to_axis(pitch_dz)       # forward tilt is negative pitch

                gamepad.left_joystick(x_value=x_axis, y_value=-y_axis)  # update the left stick position

                if button:
                    gamepad.press_button(vg.XUSB_BUTTON.XUSB_GAMEPAD_A)   # button held = A pressed
                else:
                    gamepad.release_button(vg.XUSB_BUTTON.XUSB_GAMEPAD_A) # button released = A released
                gamepad.update()  # send the updated state to windows

                # if we were holding mouse click when we switched to racing mode, release it
                if mouse_pressed_state:
                    mouse.release()
                    mouse_pressed_state = False

            elif mode == "M":
                # Mouse mode 
                dx = roll_dz * MOUSE_SENSITIVITY * dt    # pixels to move horizontally this frame
                dy = -pitch_dz * MOUSE_SENSITIVITY * dt  # forward tilt moves cursor up (negative y)
                if abs(dx) >= 1 or abs(dy) >= 1:
                    mouse.move(int(dx), int(dy), absolute=False, duration=0)  # move the cursor relatively

                # handle left-click based on button state
                want_pressed = bool(button)
                if want_pressed and not mouse_pressed_state:
                    mouse.press()               # button just pressed - start holding click
                    mouse_pressed_state = True
                elif not want_pressed and mouse_pressed_state:
                    mouse.release()             # button just released - let go of click
                    mouse_pressed_state = False

                # center the gamepad stick so it doesn't interfere while in mouse mode
                gamepad.left_joystick(x_value=0, y_value=0)
                gamepad.release_button(vg.XUSB_BUTTON.XUSB_GAMEPAD_A)
                gamepad.update()

            # print a status line to the terminal once per second
            if now - last_status_print >= 1.0:
                mode_name = "Racing" if mode == "R" else "Mouse" if mode == "M" else mode
                btn_str = "[BTN]" if button else "     "
                print(
                    f"\r{mode_name:>6} | pitch={pitch:+7.2f}° roll={roll:+7.2f}° {btn_str}",
                    end="",
                    flush=True,
                )
                last_status_print = now

    except KeyboardInterrupt:
        print("\nShutting down ...")
    finally:
        # clean up before exiting - reset gamepad and release any held mouse button
        gamepad.reset()
        gamepad.update()
        if mouse_pressed_state:
            mouse.release()
        ser.close()
        print("Goodbye.")


if __name__ == "__main__":
    main()  # only runs if we execute this file directly, not if it's imported