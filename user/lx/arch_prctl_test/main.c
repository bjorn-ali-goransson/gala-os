/* arch_prctl_test: arch_prctl(2)'s FS and GS bases and thread-local
 * storage through the thread pointer (the System V AMD64 TLS ABI, variant
 * II: the TLS block lies below the thread pointer, whose first word points
 * to itself).
 *
 * musl sets FS at start-up (its own arch_prctl(ARCH_SET_FS)); while the
 * test points FS elsewhere it calls nothing (build.sh turns the stack
 * protector off: its canary is read from %fs:0x28). */
#include "../common/lx.h"

#define ARCH_SET_GS 0x1001
#define ARCH_SET_FS 0x1002
#define ARCH_GET_FS 0x1003
#define ARCH_GET_GS 0x1004

static __thread int tls_value = 1234;
static __thread int tls_zero;

static u64 fs_word0(void)
{
	u64 v;
	__asm__ volatile("mov %%fs:0, %0" : "=r"(v));
	return v;
}

static u64 fs_word(u64 offset)
{
	u64 v;
	__asm__ volatile("mov %%fs:(%1), %0" : "=r"(v) : "r"(offset));
	return v;
}

static u64 gs_word(u64 offset)
{
	u64 v;
	__asm__ volatile("mov %%gs:(%1), %0" : "=r"(v) : "r"(offset));
	return v;
}

static u64 block[64] __attribute__((aligned(64)));

int main(void)
{
	u64 fs = 0, gs = 1;
	say("ARCH_GET_FS", sys2(NR_arch_prctl, ARCH_GET_FS, &fs));
	say_bool("FS set by musl", fs != 0);
	say_bool("%fs:0 is the thread pointer", fs_word0() == fs);
	say("tls_value", tls_value);
	say("tls_zero", tls_zero);
	say_bool("TLS below the thread pointer", (u64)&tls_value < fs && (u64)&tls_zero < fs);
	tls_value = 99;
	say("tls_value after a store", tls_value);

	say("ARCH_GET_GS", sys2(NR_arch_prctl, ARCH_GET_GS, &gs));
	say("GS base at start", gs);

	/* Point FS at our own block: %fs:0 is its self pointer, %fs:8 a mark. */
	block[0] = (u64)block;
	block[1] = 0x5eed;
	say("ARCH_SET_FS(block)", sys2(NR_arch_prctl, ARCH_SET_FS, block));
	u64 w0 = fs_word0(), w1 = fs_word(8), got = 0;
	sys2(NR_arch_prctl, ARCH_GET_FS, &got);
	say("ARCH_SET_FS(musl's) back", sys2(NR_arch_prctl, ARCH_SET_FS, fs));
	say_bool("%fs:0 was the block", w0 == (u64)block);
	say_hex("%fs:8", w1);
	say_bool("ARCH_GET_FS gave the block", got == (u64)block);
	say("tls_value with musl's FS back", tls_value);

	say("ARCH_SET_GS(block)", sys2(NR_arch_prctl, ARCH_SET_GS, block));
	say_hex("%gs:8", gs_word(8));
	got = 0;
	sys2(NR_arch_prctl, ARCH_GET_GS, &got);
	say_bool("ARCH_GET_GS gave the block", got == (u64)block);
	say("ARCH_SET_GS(0)", sys2(NR_arch_prctl, ARCH_SET_GS, 0));

	say("ARCH_SET_FS(non-canonical)", sys2(NR_arch_prctl, ARCH_SET_FS, 0x8000000000000000UL));
	say("ARCH_SET_FS(kernel address)", sys2(NR_arch_prctl, ARCH_SET_FS, 0xffff800000000000UL));
	/* User space ends at TASK_SIZE_MAX: 2^47 - 4096 with 4-level paging,
	 * 2^56 - 4096 where the kernel uses 5-level paging (LA57: one of CI's
	 * runners). A fixed mapping just above 2^47 tells which, so the
	 * transcript is the same on both. */
	u64 top = 0x7ffffffff000UL;
	{
		i64 m = sys6(NR_mmap, (i64)0x800000000000UL, 4096, 1 /* PROT_READ */,
			     0x22 | 0x100000 /* MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE */, -1, 0);
		if (m == (i64)0x800000000000UL) {
			top = 0xfffffffffff000UL;
			sys2(NR_munmap, m, 4096);
		}
	}
	say("ARCH_SET_FS(top of user space)", sys2(NR_arch_prctl, ARCH_SET_FS, top));
	{
		i64 last = sys2(NR_arch_prctl, ARCH_SET_FS, top - 4096);
		i64 back = sys2(NR_arch_prctl, ARCH_SET_FS, fs);
		say("ARCH_SET_FS(last user page)", last);
		say("ARCH_SET_FS(musl's) again", back);
	}
	say("ARCH_GET_FS(bad pointer)", sys2(NR_arch_prctl, ARCH_GET_FS, 0x10));
	say("arch_prctl(unknown code)", sys2(NR_arch_prctl, 0x1999, 0));
	say("arch_prctl(0)", sys2(NR_arch_prctl, 0, 0));

	say_text("arch_prctl_test: ok");
	exit_group(0);
}
