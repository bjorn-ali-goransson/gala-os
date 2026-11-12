#!/bin/sh
# A distribution's desktop in one window (ROADMAP LD step 6): rootful
# Xwayland on display :1 as one window of shasha's desktop (framed by
# shasha through xdg-decoration: -decorate asks libdecor, which takes the
# server's decorations), twm as its window manager with gala's twmrc,
# and an xterm. The session ends when that xterm does.
# Run as: sh /lx/xdesktop/session.sh (WAYLAND_DISPLAY, XDG_RUNTIME_DIR set);
# Xwayland's and twm's output go to $XDG_RUNTIME_DIR/xdesktop.log.
log=${XDG_RUNTIME_DIR:-/tmp}/xdesktop.log
# Xwayland's and twm's output, on ours (with dash's read: a distribution
# may have no cat).
show_log() {
	echo "session: $log:"
	while IFS= read -r line; do echo "  $line"; done <"$log"
}
# Xwayland writes the display's number to fd 3 once it accepts clients
# (-displayfd): its socket file is there before it listens.
ready=${XDG_RUNTIME_DIR:-/tmp}/xdesktop.ready
: >"$ready"
Xwayland :1 -geometry 1024x640 -shm -decorate -noreset -displayfd 3 >"$log" 2>&1 3>"$ready" &
xw=$!
# Until Xwayland accepts clients.
i=0
while [ ! -s "$ready" ] && [ $i -lt 600 ]; do
	# Xwayland gone: its output says why (the session ends).
	if ! kill -0 $xw 2>/dev/null; then
		echo "session: Xwayland ended before :1 was ready"
		show_log
		exit 1
	fi
	sleep 0.1
	i=$((i + 1))
done
echo "session: display :1 ready"
DISPLAY=:1 twm -f /lx/xdesktop/twmrc >>"$log" 2>&1 &
wm=$!
DISPLAY=:1 xterm -geometry 80x24+40+40
status=$?
echo "session: xterm ended"
# A session that failed (xterm could not start, or Xwayland had ended):
# Xwayland's and twm's output say why.
if ! kill -0 $xw 2>/dev/null; then
	echo "session: Xwayland had ended"
	show_log
elif [ $status -ne 0 ]; then
	echo "session: xterm failed ($status)"
	show_log
fi
kill $wm $xw 2>/dev/null
wait $wm 2>/dev/null
wait $xw 2>/dev/null
echo "session: done"
