# The "gtk3" root's workload, part 2: gtk_test over X11 (Xvfb). cp user/lx/bin/gtk_test /work.
export HOME=/work/home
mkdir -p $HOME && cd /work
Xvfb :9 -screen 0 1024x768x24 >/work/xvfb.log 2>&1 &
sleep 3
DISPLAY=:9 GDK_BACKEND=x11 strace -f -qq -y -o /work/t-gtk-x11.txt -e trace=%file,execve ./gtk_test >/work/o-x11.txt 2>&1; echo "x11: exit $?"
kill %1
