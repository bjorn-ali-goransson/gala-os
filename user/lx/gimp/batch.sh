# GIMP in batch mode (ROADMAP LD step 9, scenario lx_gimp_batch): the
# console GIMP of Ubuntu 24.04 (LINUX_ROOTS "gimp") opens in.png, blurs
# and inverts it and saves out.png (batch.scm), and the file's SHA-256 is
# printed: the transcript (user/lx/sh/lx_gimp_batch.txt) is this script's
# run on Ubuntu 24.04 with the same packages, so the hash is real Linux's.
# In gala the busybox applets come from /lx/busybox; on the host run it
# with BB= (the host's coreutils) and DIR=<this directory>.
BB=${BB-/lx/busybox}
DIR=${DIR-/lx/gimp}
cd "$HOME" || exit 1
$BB cp "$DIR/in.png" in.png || exit 1
gimp-console-2.10 -n -i -d -f -s --gimprc="$DIR/gimprc" \
    --batch-interpreter=plug-in-script-fu-eval \
    -b "(load \"$DIR/batch.scm\")" -b '(gimp-quit 0)'
echo "gimp-console-2.10: exit status $?"
$BB sha256sum out.png
