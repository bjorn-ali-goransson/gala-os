/* auxv_test: the initial process stack a Linux program starts with
 * (System V AMD64 ABI, "Process Initialization"), as real Linux lays it
 * out: argc, argv, envp, the auxiliary vector, the strings and the 16
 * random bytes, and the relations that do not vary between runs (the
 * stack's top and the gap above AT_PLATFORM are randomized).
 *
 * `cargo xtask lx-expect` runs it as `./auxv_test first '' 'third arg'`
 * with the environment A=1, EMPTY=, LONGER_NAME=some value and nothing
 * else, from a directory holding it under that name (the static-pie too),
 * so the string area is the same for every run and build. The kernel's
 * host tests (kernel/src/lx/stack.rs) build the stack for the same
 * arguments and compare it with this transcript byte for byte.
 *
 * The stack pointer at entry is &argc: musl's start-up code passes argv
 * on, which is sp + 8. */
#include "../common/lx.h"

#define AT_NULL 0
#define AT_PHDR 3
#define AT_PHENT 4
#define AT_PHNUM 5
#define AT_PAGESZ 6
#define AT_BASE 7
#define AT_FLAGS 8
#define AT_ENTRY 9
#define AT_UID 11
#define AT_EUID 12
#define AT_GID 13
#define AT_EGID 14
#define AT_PLATFORM 15
#define AT_HWCAP 16
#define AT_CLKTCK 17
#define AT_SECURE 23
#define AT_RANDOM 25
#define AT_EXECFN 31

struct ehdr {
	unsigned char ident[16];
	unsigned short type, machine;
	unsigned version;
	u64 entry, phoff, shoff;
	unsigned flags;
	unsigned short ehsize, phentsize, phnum, shentsize, shnum, shstrndx;
};

struct phdr {
	unsigned type, flags;
	u64 offset, vaddr, paddr, filesz, memsz, align;
};

extern const struct ehdr __ehdr_start;

static u64 len(const char *s)
{
	u64 n = 0;
	while (s[n])
		n++;
	return n;
}

static int same(const char *a, const char *b)
{
	while (*a && *a == *b)
		a++, b++;
	return *a == *b;
}

static const char *at_name(u64 type)
{
	switch (type) {
	case AT_PHDR: return "PHDR";
	case AT_PHENT: return "PHENT";
	case AT_PHNUM: return "PHNUM";
	case AT_PAGESZ: return "PAGESZ";
	case AT_BASE: return "BASE";
	case AT_FLAGS: return "FLAGS";
	case AT_ENTRY: return "ENTRY";
	case AT_UID: return "UID";
	case AT_EUID: return "EUID";
	case AT_GID: return "GID";
	case AT_EGID: return "EGID";
	case AT_PLATFORM: return "PLATFORM";
	case AT_HWCAP: return "HWCAP";
	case AT_CLKTCK: return "CLKTCK";
	case AT_SECURE: return "SECURE";
	case AT_RANDOM: return "RANDOM";
	case AT_EXECFN: return "EXECFN";
	}
	return 0;
}

static void say_str(const char *name, int i, const char *s)
{
	put(name);
	put("[");
	put_dec(i);
	put("] = \"");
	put(s);
	put("\"");
	end_line();
}

static void say_at(const char *name, int i, u64 end, u64 addr)
{
	put(name);
	if (i >= 0) {
		put("[");
		put_dec(i);
		put("]");
	}
	put(" at end-");
	put_dec(end - addr);
	end_line();
}

int main(int argc, char **argv, char **envp)
{
	u64 *sp = (u64 *)argv - 1;
	char **ev = envp;
	int envc = 0;
	while (ev[envc])
		envc++;
	u64 *auxv = (u64 *)(ev + envc + 1);
	u64 at[64] = {0};
	int naux = 0;
	put("auxv order (the types checked):");
	for (u64 *a = auxv;; a += 2, naux++) {
		if (a[0] < 64)
			at[a[0]] = a[1];
		if (a[0] == AT_NULL) {
			put(" NULL");
			break;
		}
		const char *name = at_name(a[0]);
		if (name) {
			put(" ");
			put(name);
		}
	}
	end_line();
	u64 *auxv_end = auxv + 2 * (naux + 1);

	say("argc", argc);
	say("*sp", (i64)sp[0]);
	say("sp % 16", (u64)sp % 16);
	for (int i = 0; i < argc; i++)
		say_str("argv", i, argv[i]);
	say_bool("argv[argc] NULL", argv[argc] == 0);
	for (int i = 0; i < envc; i++)
		say_str("envp", i, envp[i]);
	say_bool("envp right after argv's NULL", envp == argv + argc + 1);

	/* The string area: argv's strings, envp's, AT_EXECFN's, then 8 zero
	 * bytes, then a page boundary (the stack's end). */
	const char *execfn = (const char *)at[AT_EXECFN];
	u64 end = (u64)execfn + len(execfn) + 1 + 8;
	say("end % 4096", end % 4096);
	say("the end marker", *(u64 *)(end - 8));
	say_bool("AT_EXECFN is argv[0]'s text", same(execfn, argv[0]));
	for (int i = 0; i < argc; i++)
		say_at("argv", i, end, (u64)argv[i]);
	for (int i = 0; i < envc; i++)
		say_at("envp", i, end, (u64)envp[i]);
	say_at("AT_EXECFN", -1, end, (u64)execfn);
	put("string area =");
	for (const unsigned char *b = (const unsigned char *)argv[0]; (u64)b < end; b++) {
		put_char(' ');
		put_char("0123456789abcdef"[*b >> 4]);
		put_char("0123456789abcdef"[*b & 15]);
	}
	end_line();

	/* Below the strings (and a random gap): AT_PLATFORM's string ending on
	 * a 16-byte boundary, AT_RANDOM's 16 bytes right below it, then the
	 * table (argc .. AT_NULL) placed so that sp is 16-byte aligned. */
	const char *platform = (const char *)at[AT_PLATFORM];
	const unsigned char *random = (const unsigned char *)at[AT_RANDOM];
	u64 platform_end = (u64)platform + len(platform) + 1;
	say_str("AT_PLATFORM", 0, platform);
	say_bool("AT_PLATFORM's end 16-byte aligned", platform_end % 16 == 0);
	say_bool("AT_PLATFORM below the strings", platform_end <= (u64)argv[0]);
	say_bool("AT_RANDOM right below AT_PLATFORM", (u64)random + 16 == (u64)platform);
	{
		int nonzero = 0;
		for (int i = 0; i < 16; i++)
			nonzero |= random[i];
		say_bool("AT_RANDOM's bytes not all zero", nonzero);
	}
	u64 table = (u64)auxv_end - (u64)sp;
	/* (The table's size depends on how many auxv entries the kernel
	 * gives, which changes between versions: not printed.) */
	say_bool("auxv's end padded the least below AT_RANDOM",
		 (u64)auxv_end <= (u64)random && (u64)sp == (((u64)random - table) & ~15UL));

	/* The values: page size, the program headers, the entry point, ids. */
	const struct ehdr *eh = &__ehdr_start;
	const struct phdr *ph = (const struct phdr *)((const char *)eh + eh->phoff);
	u64 bias = 0;
	for (int i = 0; i < eh->phnum; i++)
		if (ph[i].type == 1 && ph[i].offset == 0)
			bias = (u64)eh - ph[i].vaddr;
	say("AT_PAGESZ", at[AT_PAGESZ]);
	say("AT_PHENT", at[AT_PHENT]);
	say_bool("AT_PHNUM is e_phnum", at[AT_PHNUM] == eh->phnum);
	say_bool("AT_PHDR is the headers' address", at[AT_PHDR] == (u64)eh + eh->phoff);
	say_bool("AT_ENTRY is e_entry + the load bias", at[AT_ENTRY] == eh->entry + bias);
	say_bool("ET_DYN loaded at a non-zero bias", eh->type == 3 ? bias != 0 : bias == 0);
	say("AT_BASE", at[AT_BASE]);
	say("AT_FLAGS", at[AT_FLAGS]);
	say_bool("AT_UID is getuid()", at[AT_UID] == (u64)sys0(NR_getuid));
	say_bool("AT_EUID is geteuid()", at[AT_EUID] == (u64)sys0(NR_geteuid));
	say_bool("AT_GID is getgid()", at[AT_GID] == (u64)sys0(NR_getgid));
	say_bool("AT_EGID is getegid()", at[AT_EGID] == (u64)sys0(NR_getegid));
	say("AT_SECURE", at[AT_SECURE]);
	say("AT_CLKTCK", at[AT_CLKTCK]);
	{
		unsigned a = 1, b, c = 0, d;
		__asm__ volatile("cpuid" : "+a"(a), "=b"(b), "+c"(c), "=d"(d));
		say_bool("AT_HWCAP is CPUID(1).EDX", at[AT_HWCAP] == d);
	}

	say_text("auxv_test: ok");
	exit_group(0);
}
