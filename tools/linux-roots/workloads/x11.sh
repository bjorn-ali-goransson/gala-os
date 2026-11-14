# The "x11" root's workload. Clear /var/lib/xkb/*.xkm first (Xwayland caches compiled keymaps),
# move /usr/lib/x86_64-linux-gnu/dri aside (no GLX: gala's Xwayland has none), and copy
# user/lx/xdesktop/{session.sh,twmrc} to /lx/xdesktop.
# Xwayland as gala runs it (rootless, -shm) with an xterm whose shell exits;
# then user/lx/xdesktop/session.sh itself (its xterm ended after a while).
export HOME=/work/home XDG_RUNTIME_DIR=/work/xdgb
mkdir -p $HOME $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
Xvfb :4 -nolisten tcp -screen 0 1280x800x24 >/work/xvfbb.log 2>&1 &
sleep 2
DISPLAY=:4 weston --backend=x11 --use-pixman --idle-time=0 --socket=w4 >/work/westonb.log 2>&1 &
sleep 3
export WAYLAND_DISPLAY=w4
strace -f -qq -y -o /work/tb-xw.txt -e trace=%file,execve Xwayland :5 -rootless -shm -noreset >/work/xwb.log 2>&1 &
sleep 4
DISPLAY=:5 SHELL=/bin/sh strace -f -qq -y -o /work/tb-xterm.txt -e trace=%file,execve xterm -e /bin/sh -c 'echo hi; exit 0'; echo "xterm: exit $?"
SHELL=/bin/sh strace -f -qq -y -o /work/tb-session.txt -e trace=%file,execve sh /lx/xdesktop/session.sh >/work/session.out 2>&1 &
sleep 10
pkill -x xterm; sleep 2
cat /work/session.out
pkill Xwayland; pkill weston; pkill Xvfb
