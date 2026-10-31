# busybox utilities over files (H3), run as `busybox sh files.sh`.
# Every result is printed on one labelled line; the expected lines are
# the same on Windows and on gala (the `busybox_files` scenario).
set -e
d="${TEMP:-/tmp}/gala_files"
rm -rf "$d"
mkdir -p "$d/a/b"
cd "$d"
echo one > a/1.txt
echo two > a/b/2.txt
printf 'three\nfour\n' > a/b/3.txt
echo "ls: $(ls a | tr '\n' ' ')"
echo "ls -l: $(ls -l a | grep -c txt) file"
echo "find: $(find a | sort | tr '\n' ' ')"
echo "find -type f: $(find a -type f | wc -l | tr -d ' ')"
echo "grep -r: $(grep -r four a)"
echo "wc -l: $(cat a/b/3.txt | wc -l | tr -d ' ')"
cp -r a c
echo "cp -r: $(find c -type f | sort | tr '\n' ' ')"
echo "cmp: $(cmp a/b/3.txt c/b/3.txt && echo same)"
echo "md5sum: $(md5sum a/1.txt)"
tar cf x.tar a
echo "tar tf: $(tar tf x.tar | sort | tr '\n' ' ')"
mkdir u
(cd u && tar xf ../x.tar)
echo "tar xf: $(cat u/a/b/3.txt | tr '\n' ' ')"
rm -r a c u x.tar
echo "rm -r: [$(ls)]"
cd ..
rmdir "$d"
echo "files: ok"
