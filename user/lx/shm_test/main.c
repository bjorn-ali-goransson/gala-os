/* shm_test: System V shared memory, shmget(2), shmat(2), shmdt(2) and
 * shmctl(2) (ROADMAP LD step 9: GIMP hands image tiles to its plug-ins
 * in a segment, and X's MIT-SHM extension uses them; ENOSYS in gala).
 *
 * GIMP's use: a private segment made, attached, removed at once
 * (IPC_RMID) and still used and attached by id from its plug-ins (Linux
 * destroys a removed segment at its last detach). The test prints
 * relations that hold on any machine (ids, times and pids differ): the
 * segment's pages shared between attachments and through a fork, the
 * attach count IPC_STAT reports (a fork's child counted, a detach and an
 * exit not), the deferred removal, keys and their errors. Nothing
 * depends on being root: no permission checks. */
#include "../common/lx.h"
#include "../common/more.h"

#define IPC_CREAT 01000
#define IPC_EXCL 02000
#define IPC_RMID 0
#define IPC_SET 1
#define IPC_STAT 2
#define SHM_DEST 01000
#define SHM_RND 020000

/* struct shmid64_ds on x86-64 (shmctl(2)): 112 bytes. */
struct lx_shmid_ds {
	int key;
	unsigned uid, gid, cuid, cgid, mode;
	unsigned short seq, pad2;
	u64 unused1, unused2;
	u64 segsz;
	i64 atime, dtime, ctime;
	int cpid, lpid;
	u64 nattch, unused4, unused5;
};

static struct lx_shmid_ds st;

static i64 stat_of(i64 id)
{
	lx_memset(&st, 0x55, sizeof st);
	return sys3(SYS_shmctl, id, IPC_STAT, &st);
}

int main(void)
{
	lx_init();
	say("sizeof(struct shmid64_ds)", sizeof(struct lx_shmid_ds));
	i64 me = sys0(SYS_getpid);

	say_text("-- a private segment");
	i64 id = sys3(SYS_shmget, 0 /* IPC_PRIVATE */, 10000, IPC_CREAT | 0600);
	say_bool("shmget(IPC_PRIVATE, 10000) >= 0", id >= 0);
	say_ret("IPC_STAT", stat_of(id));
	say("  shm_segsz", st.segsz);
	say_oct("  mode", st.mode);
	say("  key", st.key);
	say("  shm_nattch", st.nattch);
	say_bool("  shm_cpid is ours", st.cpid == me);
	say("  shm_lpid", st.lpid);
	say("  shm_atime", st.atime);
	say("  shm_dtime", st.dtime);
	say_bool("  shm_ctime > 0", st.ctime > 0);
	say_bool("  uid and cuid ours", st.uid == sys0(SYS_geteuid) && st.cuid == st.uid);
	say_bool("  gid and cgid ours", st.gid == sys0(SYS_getegid) && st.cgid == st.gid);

	i64 a = sys3(SYS_shmat, id, 0, 0);
	say_bool("shmat(id) is a page", a > 0 && a % 4096 == 0);
	char *p = (char *)a;
	say_bool("  a new segment's bytes are zero", p[0] == 0 && p[9999] == 0 && p[12287] == 0);
	p[0] = 'g';
	p[9999] = 'p';
	i64 b = sys3(SYS_shmat, id, 0, 0);
	say_bool("a second shmat: another address", b > 0 && b != a);
	char *q = (char *)b;
	say_bool("  the same bytes", q[0] == 'g' && q[9999] == 'p');
	q[1] = 'i';
	say_bool("  writes seen through the first", p[1] == 'i');
	stat_of(id);
	say("  shm_nattch", st.nattch);
	say_bool("  shm_lpid is ours", st.lpid == me);
	say_bool("  shm_atime > 0", st.atime > 0);

	say_text("-- a fork");
	i64 pid = lx_fork();
	if (pid == 0) {
		/* The child: its attachments are counted; it writes. */
		stat_of(id);
		int counted = st.nattch == 4;
		((char *)a)[2] = 'm';
		lx_exit(counted ? 0 : 1);
	}
	say_status("the child (it saw shm_nattch 4)", wait_for(pid, 0));
	say_bool("its write seen", p[2] == 'm' && q[2] == 'm');
	stat_of(id);
	say("shm_nattch after its exit", st.nattch);

	say_text("-- detaching");
	say_ret("shmdt(the second)", sys1(SYS_shmdt, b));
	stat_of(id);
	say("  shm_nattch", st.nattch);
	say_bool("  shm_dtime > 0", st.dtime > 0);
	say_ret("shmdt(it again)", sys1(SYS_shmdt, b));
	say_ret("shmdt(inside the first)", sys1(SYS_shmdt, a + 4096));
	say_ret("shmdt(not a page)", sys1(SYS_shmdt, a + 1));

	say_text("-- removed while attached (what GIMP does)");
	say_ret("IPC_RMID", sys3(SYS_shmctl, id, IPC_RMID, 0));
	say_ret("IPC_STAT", stat_of(id));
	say_bool("  SHM_DEST in its mode", (st.mode & SHM_DEST) != 0);
	say_oct("  the permissions", st.mode & 0777);
	say("  key", st.key);
	say_bool("  still usable", p[0] == 'g' && p[2] == 'm');
	i64 c = sys3(SYS_shmat, id, 0, 0);
	say_bool("shmat(it) still works", c > 0 && c % 4096 == 0 && ((char *)c)[0] == 'g');
	stat_of(id);
	say("  shm_nattch", st.nattch);
	say_ret("shmdt", sys1(SYS_shmdt, c));
	say_ret("shmdt(the last)", sys1(SYS_shmdt, a));
	say_ret("IPC_STAT after the last detach", stat_of(id));
	say_ret("shmat after the last detach", sys3(SYS_shmat, id, 0, 0));
	say_ret("IPC_RMID after the last detach", sys3(SYS_shmctl, id, IPC_RMID, 0));

	say_text("-- a key");
	int key = 0x6a6c0000 | (int)(me & 0xffff);
	i64 k = sys3(SYS_shmget, key, 8192, IPC_CREAT | IPC_EXCL | 0640);
	say_bool("shmget(key, IPC_CREAT|IPC_EXCL) >= 0", k >= 0);
	say_ret("again with IPC_EXCL", sys3(SYS_shmget, key, 8192, IPC_CREAT | IPC_EXCL | 0640));
	say_bool("again without it: the same id", sys3(SYS_shmget, key, 8192, IPC_CREAT | 0640) == k);
	say_bool("without IPC_CREAT: the same id", sys3(SYS_shmget, key, 0, 0) == k);
	say_bool("a smaller size: the same id", sys3(SYS_shmget, key, 100, 0) == k);
	say_ret("a larger size", sys3(SYS_shmget, key, 8193, 0));
	stat_of(k);
	say_bool("IPC_STAT: its key", st.key == key);
	say_oct("  mode", st.mode);
	say("  shm_segsz", st.segsz);
	i64 other = sys3(SYS_shmget, 0, 4096, IPC_CREAT | 0600);
	say_bool("another private segment: another id", other >= 0 && other != k);
	sys3(SYS_shmctl, other, IPC_RMID, 0);
	say_ret("IPC_RMID with no attachment", sys3(SYS_shmctl, k, IPC_RMID, 0));
	say_ret("IPC_STAT after it", stat_of(k));
	say_ret("the key after it, without IPC_CREAT", sys3(SYS_shmget, key, 0, 0));

	say_text("-- errors");
	say_ret("shmget(size 0)", sys3(SYS_shmget, 0, 0, IPC_CREAT | 0600));
	say_ret("shmget(no such key)", sys3(SYS_shmget, key ^ 0x8000, 4096, 0600));
	say_ret("shmat(-1)", sys3(SYS_shmat, -1, 0, 0));
	say_ret("shmat(no such id)", sys3(SYS_shmat, 0x7ffffff0, 0, 0));
	i64 e = sys3(SYS_shmget, 0, 4096, IPC_CREAT | 0600);
	say_ret("shmat(an address not a page)", sys3(SYS_shmat, e, 0x10000001, 0));
	i64 r = sys3(SYS_shmat, e, 0x20000001, SHM_RND);
	say_bool("shmat(the same, SHM_RND): rounded down", r == 0x20000000);
	sys1(SYS_shmdt, r);
	say_ret("IPC_STAT(bad buffer)", sys3(SYS_shmctl, e, IPC_STAT, 1));
	say_ret("shmctl(unknown command)", sys3(SYS_shmctl, e, 99, &st));
	say_ret("shmctl(no such id)", sys3(SYS_shmctl, 0x7ffffff0, IPC_STAT, &st));
	sys3(SYS_shmctl, e, IPC_RMID, 0);
	say_text("shm_test: ok");
	return 0;
}
