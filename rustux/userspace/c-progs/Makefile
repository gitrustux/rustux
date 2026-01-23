# Makefile for Rustux C userspace programs

CC = x86_64-elf-gcc
LD = x86_64-elf-ld
OBJCOPY = x86_64-elf-objcopy

CFLAGS = -ffreestanding -nostdlib -fno-stack-protector -fno-pic -O2 -Wall -Wextra
LDFLAGS = -nostdlib -T linker.ld

# Default target
all: hello.elf counter.elf init.elf

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

# Copy binaries to expected locations
copy: all
	cp hello.elf ../../../target/hello.elf
	cp counter.elf ../../../target/counter.elf
	cp init.elf ../../../target/init.elf

# Clean build artifacts
clean:
	rm -f *.o *.elf

.PHONY: all copy clean
