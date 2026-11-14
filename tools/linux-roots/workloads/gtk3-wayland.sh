# The "gtk3" root's workload, part 1 (run in the chroot as uid 1000; cp user/lx/bin/gtk_test /work):
# gtk_test over Wayland as lx-expect runs it (weston's X11 backend in Xvfb),
# as an unprivileged user (gala's programs run as uid 1000), fully traced.
export HOME=/work/u/home XDG_RUNTIME_DIR=/work/u/xdg
mkdir -p $HOME $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
Xvfb :7 -nolisten tcp -screen 0 1280x800x24 >/work/u/xvfb.log 2>&1 &
sleep 2
DISPLAY=:7 weston --backend=x11 --use-pixman --idle-time=0 --socket=w7 >/work/u/weston.log 2>&1 &
sleep 3
cd /work
env -i HOME=$HOME XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR WAYLAND_DISPLAY=w7 A=1 EMPTY= LONGER_NAME="some value" \
  strace -f -qq -y -o /work/u/trace.txt ./gtk_test >/work/u/out.txt 2>/work/u/err.txt; echo "wayland: exit $?"
kill %2 %1
