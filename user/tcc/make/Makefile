# The tcc_make scenario (user/tcc/make.sh) runs busybox's make here
# with CC and CFLAGS naming the unpacked TinyCC.
CC = tcc
CFLAGS =
OBJS = main.o add.o mul.o

calc.exe: $(OBJS)
	$(CC) $(CFLAGS) -o calc.exe $(OBJS)

main.o: main.c calc.h
	$(CC) $(CFLAGS) -c main.c

add.o: add.c calc.h
	$(CC) $(CFLAGS) -c add.c

mul.o: mul.c calc.h
	$(CC) $(CFLAGS) -c mul.c

# A rule that fails: make stops with an error status.
broken:
	$(CC) $(CFLAGS) -c missing.c
