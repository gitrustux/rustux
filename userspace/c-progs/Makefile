# Makefile for Rustux C userspace programs

CC = x86_64-elf-gcc
LD = x86_64-elf-ld
OBJCOPY = x86_64-elf-objcopy

CFLAGS = -ffreestanding -nostdlib -fno-stack-protector -fno-pic -O2 -Wall -Wextra
LDFLAGS = -nostdlib -T linker.ld

# Default target
all: hello.elf counter.elf init.elf objtest.elf ipctest.elf spawntest.elf true.elf ps2kbd.elf drvtest.elf smptest.elf

# Build hello.elf
hello.elf: hello.o
	$(LD) $(LDFLAGS) -o $@ $<

hello.o: hello.c syscall.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build counter.elf
counter.elf: counter.o
	$(LD) $(LDFLAGS) -o $@ $<

counter.o: counter.c syscall.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build init.elf
init.elf: init.o
	$(LD) $(LDFLAGS) -o $@ $<

init.o: init.c syscall.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build objtest.elf (object syscall tests)
objtest.elf: objtest.o
	$(LD) $(LDFLAGS) -o $@ $<

objtest.o: objtest.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build ipctest.elf (processes, threads, jobs)
ipctest.elf: ipctest.o
	$(LD) $(LDFLAGS) -o $@ $<

ipctest.o: ipctest.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build spawntest.elf and true.elf (process cleanup)
spawntest.elf: spawntest.o
	$(LD) $(LDFLAGS) -o $@ $<

spawntest.o: spawntest.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

true.elf: true.o
	$(LD) $(LDFLAGS) -o $@ $<

true.o: true.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build ps2kbd.elf (PS/2 keyboard driver) and drvtest.elf
ps2kbd.elf: ps2kbd.o
	$(LD) $(LDFLAGS) -o $@ $<

ps2kbd.o: ps2kbd.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

drvtest.elf: drvtest.o
	$(LD) $(LDFLAGS) -o $@ $<

drvtest.o: drvtest.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

# Build smptest.elf (threads on several CPUs)
smptest.elf: smptest.o
	$(LD) $(LDFLAGS) -o $@ $<

smptest.o: smptest.c rx.h
	$(CC) $(CFLAGS) -c $< -o $@

# Copy binaries to expected locations
copy: all
	cp hello.elf ../../../target/hello.elf
	cp counter.elf ../../../target/counter.elf
	cp init.elf ../../../target/init.elf

# Clean build artifacts
clean:
	rm -f *.o *.elf

.PHONY: all copy clean
