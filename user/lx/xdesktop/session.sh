#!/bin/sh
# A distribution's desktop in one window (ROADMAP LD step 6): rootful
# Xwayland on display :1 as one window of shasha's desktop (framed by
# shasha through xdg-decoration: -decorate asks libdecor, which takes the
# server's decorations), twm as its window manager with gala's twmrc,
# and an xterm. The session ends when that xterm does.
# Run as: sh /lx/xdesktop/session.sh (WAYLAND_DISPLAY, XDG_RUNTIME_DIR set);
# Xwayland's and twm's output go to $XDG_RUNTIME_DIR/xdesktop.log.
log=${XDG_RUNTIME_DIR:-/tmp}/xdesktop.log
Xwayland :1 -geometry 1024x640 -shm -decorate -noreset >"$log" 2>&1 &
xw=$!
# Until Xwayland listens (its socket is there).
i=0
while [ ! -e /tmp/.X11-unix/X1 ] && [ $i -lt 600 ]; do
	sleep 0.1
	i=$((i + 1))
done
echo "session: display :1 ready"
DISPLAY=:1 twm -f /lx/xdesktop/twmrc >>"$log" 2>&1 &
wm=$!
DISPLAY=:1 xterm -geometry 80x24+40+40
echo "session: xterm ended"
kill $wm $xw 2>/dev/null
wait $wm 2>/dev/null
wait $xw 2>/dev/null
echo "session: done"
