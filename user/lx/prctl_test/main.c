/* prctl_test: prctl(2)'s thread name (PR_SET_NAME, PR_GET_NAME) and
 * capability bounding set (PR_CAPBSET_READ), and getresuid(2) and
 * getresgid(2), as GLib, libcap and libselinux use them when a GTK program
 * starts (ROADMAP LD step 3: each was ENOSYS in gala).
 *
 * The bounding set is probed only where every system agrees: capability
 * 0 (CAP_CHOWN) is in it (a container keeps it too), 63 is past the last
 * capability any kernel has (EINVAL, which libcap's probe for the last one
 * relies on). The name a process starts with is its executable's file
 * name cut to 15 bytes (the -pie binary's differs: printed as a relation
 * with /proc/self/exe). The ids are printed as relations, not numbers. */
#include <pthread.h>

#include "../common/lx.h"
#include "../common/more.h"

#define PR_SET_NAME 15
#define PR_GET_NAME 16
#define PR_CAPBSET_READ 23

/* The name prctl reports: 16 bytes, NUL-terminated (filled with 'x'
 * first, so a missing NUL shows). */
static i64 get_name(char out[16])
{
	lx_memset(out, 'x', 16);
	return sys2(SYS_prctl, PR_GET_NAME, out);
}

static void say_name(const char *label, i64 r, const char *name)
{
	put(label);
	put(" = ");
	if (r < 0) {
		put_ret(r);
	} else {
		put_quoted(name, (i64)lx_strlen(name));
	}
	end_line();
}

static char thread_name[16];
static i64 thread_set, thread_get;

static void *thread_main(void *arg)
{
	(void)arg;
	thread_set = sys2(SYS_prctl, PR_SET_NAME, "worker");
	thread_get = get_name(thread_name);
	return 0;
}

int main(void)
{
	lx_init();
	char name[16];

	/* The start: the executable's file name, at most 15 bytes. */
	char exe[256];
	i64 n = sys3(SYS_readlink, "/proc/self/exe", exe, sizeof exe - 1);
	exe[n > 0 ? n : 0] = 0;
	const char *base = exe;
	for (const char *p = exe; *p; p++)
		if (*p == '/')
			base = p + 1;
	char want[16];
	lx_memset(want, 0, sizeof want);
	for (int i = 0; i < 15 && base[i]; i++)
		want[i] = base[i];
	i64 r = get_name(name);
	say_ret("PR_GET_NAME at start", r);
	say_bool("the start name is the executable's, cut to 15 bytes",
		 r == 0 && lx_streq(name, want));

	say_ret("PR_SET_NAME(\"gtk-main\")", sys2(SYS_prctl, PR_SET_NAME, "gtk-main"));
	say_name("PR_GET_NAME", get_name(name), name);
	/* Longer names are cut to 15 bytes and a NUL. */
	say_ret("PR_SET_NAME(20 bytes)", sys2(SYS_prctl, PR_SET_NAME, "abcdefghijklmnopqrst"));
	say_name("PR_GET_NAME", get_name(name), name);
	say_ret("PR_SET_NAME(\"\")", sys2(SYS_prctl, PR_SET_NAME, ""));
	say_name("PR_GET_NAME", get_name(name), name);
	say_ret("PR_SET_NAME(bad pointer)", sys2(SYS_prctl, PR_SET_NAME, 1));
	say_ret("PR_GET_NAME(bad pointer)", sys2(SYS_prctl, PR_GET_NAME, 1));
	sys2(SYS_prctl, PR_SET_NAME, "main");

	/* A thread's name is its own. */
	pthread_t t;
	say_ret("pthread_create", -(i64)pthread_create(&t, 0, thread_main, 0));
	say_ret("pthread_join", -(i64)pthread_join(t, 0));
	say_ret("thread: PR_SET_NAME(\"worker\")", thread_set);
	say_name("thread: PR_GET_NAME", thread_get, thread_name);
	say_name("main: PR_GET_NAME after it", get_name(name), name);

	/* The capability bounding set. */
	say_ret("PR_CAPBSET_READ(CAP_CHOWN)", sys2(SYS_prctl, PR_CAPBSET_READ, 0));
	say_ret("PR_CAPBSET_READ(63)", sys2(SYS_prctl, PR_CAPBSET_READ, 63));
	say_ret("PR_CAPBSET_READ(-1)", sys2(SYS_prctl, PR_CAPBSET_READ, -1));
	/* An option no kernel has. */
	say_ret("prctl(12345)", sys2(SYS_prctl, 12345, 0));

	/* getresuid and getresgid: real, effective and saved, each what
	 * getuid/geteuid (getgid/getegid) give for a program that is not
	 * set-id. */
	unsigned ids[3] = { 7, 7, 7 };
	say_ret("getresuid", sys3(SYS_getresuid, &ids[0], &ids[1], &ids[2]));
	say_bool("real, effective, saved uid are getuid's",
		 ids[0] == (unsigned)sys0(SYS_getuid) && ids[1] == (unsigned)sys0(SYS_geteuid) &&
			 ids[2] == (unsigned)sys0(SYS_getuid));
	unsigned gids[3] = { 7, 7, 7 };
	say_ret("getresgid", sys3(SYS_getresgid, &gids[0], &gids[1], &gids[2]));
	say_bool("real, effective, saved gid are getgid's",
		 gids[0] == (unsigned)sys0(SYS_getgid) && gids[1] == (unsigned)sys0(SYS_getegid) &&
			 gids[2] == (unsigned)sys0(SYS_getgid));
	say_ret("getresuid(bad pointer)", sys3(SYS_getresuid, &ids[0], 1, &ids[2]));
	say_ret("getresgid(bad pointer)", sys3(SYS_getresgid, 1, &gids[1], &gids[2]));
	say_text("prctl_test: ok");
	return 0;
}
