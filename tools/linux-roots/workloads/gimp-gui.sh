# lx_gimp's transcript on Ubuntu 26.04 (WORKFLOW "GIMP"): user/lx/gimp/gui.sh
# under weston on Xvfb, driven by xdotool with the keys GIMP_SCREEN sends
# (Escape, "/" invert, Ctrl+Shift+E out.ppm, Enter, Ctrl+Q, Ctrl+D). Run in
# the "gimp" chroot as uid 1000 with user/lx/gimp copied to /work/gimp:
#   sh gimp-gui.sh <name>   (prints the transcript; screendumps in /work/u)
H=$1
export HOME=/work/u/$H XDG_RUNTIME_DIR=/work/u/xdg-$H
rm -rf $HOME $XDG_RUNTIME_DIR; mkdir -p $HOME $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
Xvfb :9 -nolisten tcp -screen 0 1280x800x24 >/work/u/xvfb9.log 2>&1 &
sleep 2
DISPLAY=:9 weston --backend=x11 --use-pixman --idle-time=0 --width=1280 --height=760 --socket=w9 >/work/u/weston9.log 2>&1 &
sleep 3
cd $HOME
env -i HOME=$HOME XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR WAYLAND_DISPLAY=w9 PATH=/usr/bin:/bin BB= DIR=/work/gimp \
  sh /work/gimp/gui.sh >/work/u/$H.out 2>&1 &
g=$!
n=0
k() { DISPLAY=:9 xdotool "$@"; sleep 2; n=$((n+1)); DISPLAY=:9 xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng >/work/u/$H.shot$n.png 2>/dev/null; }
sleep 30
DISPLAY=:9 xdotool mousemove 640 400 click 1; sleep 2
k key Escape
k key slash
DISPLAY=:9 xdotool type --delay 80 Invert; sleep 3
k key Return
k key ctrl+shift+e
k key ctrl+a
DISPLAY=:9 xdotool type --delay 50 out.ppm; sleep 1
k key Return
sleep 3
k key Return
sleep 5
k key ctrl+q
sleep 3
k key ctrl+d
sleep 3
kill -0 $g 2>/dev/null && echo "gimp still running" || { wait $g; s=$?; cat /work/u/$H.out; echo "-- exit status $s"; }
ls -la $HOME
sha256sum $HOME/out.ppm 2>/dev/null
kill $g %2 %1 2>/dev/null
