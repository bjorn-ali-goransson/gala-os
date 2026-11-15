# GIMP's window (ROADMAP LD step 9, scenario lx_gimp): Ubuntu 26.04's GIMP
# 3.2 on the session's Wayland display, opening a copy of in.png in $HOME.
# The screen check (GIMP_SCREEN) closes its welcome dialog, inverts the
# image (Colors > Invert, through "/", GIMP's action search), exports it
# as out.ppm (raw PPM: no time in the file) and quits GIMP. Its output
# (GTK's and GIMP's warnings, with times and pids) goes to $HOME/gimp.log;
# the transcript (user/lx/sh/lx_gimp.txt) is its exit status and the
# exported file's SHA-256, recorded on Ubuntu 26.04 with the same keys.
# In gala the busybox applets come from /lx/busybox; on the host run it
# with BB= (the host's coreutils) and DIR=<this directory>.
BB=${BB-/lx/busybox}
DIR=${DIR-/lx/gimp}
cd "$HOME" || exit 1
$BB cp "$DIR/in.png" in.png || exit 1
gimp-3.2 -n -s -f in.png >gimp.log 2>&1
echo "gimp-3.2: exit status $?"
$BB sha256sum out.ppm
