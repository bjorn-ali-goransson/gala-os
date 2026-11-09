/* statfs_test: statfs(2) and fstatfs(2), which fontconfig (whether a
 * cache file may be mapped), libselinux and GLib call when a GTK program
 * starts (ROADMAP LD step 3: ENOSYS in gala).
 *
 * What a file system reports differs between machines (ext4, overlayfs,
 * tmpfs; sizes and counts): the test prints relations that hold on any
 * of them. The block size is a power of two at least 512 and f_frsize
 * the same, free blocks at most the total and available at most the free,
 * names up to 255 bytes, the fsid and type the same through a path, a
 * file in it and an open descriptor; and the errors. */
#include "../common/lx.h"
#include "../common/more.h"

/* struct statfs on x86-64 (statfs(2)): 120 bytes. */
struct lx_statfs {
	i64 f_type, f_bsize;
	u64 f_blocks, f_bfree, f_bavail, f_files, f_ffree;
	int f_fsid[2];
	i64 f_namelen, f_frsize, f_flags;
	i64 f_spare[4];
};

static int pow2(i64 v)
{
	return v > 0 && (v & (v - 1)) == 0;
}

static void check(const char *label, i64 r, struct lx_statfs *s)
{
	put(label);
	put(" = ");
	put_ret(r);
	end_line();
	if (r != 0)
		return;
	say_bool("  f_bsize is a power of two >= 512", pow2(s->f_bsize) && s->f_bsize >= 512);
	say_bool("  f_frsize is f_bsize", s->f_frsize == s->f_bsize);
	say_bool("  f_bfree <= f_blocks", s->f_bfree <= s->f_blocks);
	say_bool("  f_bavail <= f_bfree", s->f_bavail <= s->f_bfree);
	say("  f_namelen", s->f_namelen);
}

int main(void)
{
	lx_init();
	say("sizeof(struct statfs)", sizeof(struct lx_statfs));
	struct lx_statfs root, here, file, open, again;
	lx_memset(&root, 0x55, sizeof root);
	check("statfs(\"/\")", sys2(SYS_statfs, "/", &root), &root);
	lx_memset(&here, 0x55, sizeof here);
	check("statfs(\".\")", sys2(SYS_statfs, ".", &here), &here);

	/* A file of our own in a directory of our own, and its descriptor. */
	sys2(SYS_mkdir, "statfs.dir", 0755);
	i64 fd = sys3(SYS_open, "statfs.dir/f", 0101 /* O_WRONLY|O_CREAT */, 0644);
	say("open", fd);
	lx_memset(&file, 0x55, sizeof file);
	say_ret("statfs(the file)", sys2(SYS_statfs, "statfs.dir/f", &file));
	lx_memset(&open, 0x55, sizeof open);
	say_ret("fstatfs(its descriptor)", sys2(SYS_fstatfs, fd, &open));
	say_bool("the same type and fsid through the path and the descriptor",
		 file.f_type == open.f_type && file.f_fsid[0] == open.f_fsid[0] &&
			 file.f_fsid[1] == open.f_fsid[1]);
	say_bool("the same type as its directory's", file.f_type == here.f_type);
	i64 dir = sys2(SYS_open, "statfs.dir", 0200000 /* O_DIRECTORY */);
	lx_memset(&again, 0x55, sizeof again);
	say_ret("fstatfs(the directory)", sys2(SYS_fstatfs, dir, &again));
	say_bool("the same type as the file's", again.f_type == file.f_type);
	sys1(SYS_close, dir);
	struct lx_statfs s;
	say_ret("statfs(through a file)", sys2(SYS_statfs, "statfs.dir/f/x", &s));
	sys1(SYS_close, fd);
	sys1(SYS_unlink, "statfs.dir/f");
	sys1(SYS_rmdir, "statfs.dir");

	say_text("-- errors");
	say_ret("statfs(missing)", sys2(SYS_statfs, "/no/such/dir", &s));
	say_ret("statfs(\"\")", sys2(SYS_statfs, "", &s));
	say_ret("statfs(bad path pointer)", sys2(SYS_statfs, 1, &s));
	say_ret("statfs(bad buffer)", sys2(SYS_statfs, "/", 1));
	say_ret("fstatfs(-1)", sys2(SYS_fstatfs, -1, &s));
	say_ret("fstatfs(99)", sys2(SYS_fstatfs, 99, &s));
	say_ret("fstatfs(bad buffer)", sys2(SYS_fstatfs, 1, 1));
	/* A pipe has a file system too (pipefs). */
	int p[2];
	sys1(SYS_pipe, p);
	say_ret("fstatfs(a pipe)", sys2(SYS_fstatfs, p[0], &s));
	say_text("statfs_test: ok");
	return 0;
}
