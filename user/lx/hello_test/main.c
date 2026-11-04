/* hello_test: the smallest Linux program, LX milestone 1's first target:
 * write(2) to stdout and exit_group(2), nothing else. */
#include "../common/lx.h"

int main(void)
{
	static const char msg[] = "hello from static musl\n";
	i64 n = sys3(NR_write, 1, msg, sizeof msg - 1);
	say("write returned", n);
	say_text("hello_test: ok");
	exit_group(0);
}
