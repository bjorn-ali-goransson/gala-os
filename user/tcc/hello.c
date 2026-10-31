/* Compiled by TinyCC on gala in the `tcc` scenario (user/tcc/tcc.sh),
   then run: printf, a loop, a function, command-line arguments and an
   exit code, each checked against what Windows gives. */
#include <stdio.h>
#include <string.h>

static int square(int x) { return x * x; }

int main(int argc, char **argv)
{
    int i, sum = 0;
    for (i = 1; i <= 10; i++)
        sum += square(i);
    printf("hello from tcc: sum of squares=%d\n", sum);
    printf("argc=%d last=%s len=%d\n", argc, argv[argc - 1],
           (int)strlen(argv[argc - 1]));
    return sum % 9;
}
