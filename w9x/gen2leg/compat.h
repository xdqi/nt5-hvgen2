/* Open Watcom: the few instructions the shim needs, as inline #pragma aux functions (no C runtime in a VxD). */
#ifndef COMPAT_H
#define COMPAT_H

typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned __int64 u64;

void outb(u16 port, u8 v);
#pragma aux outb = "out dx,al" parm [dx] [al] modify exact [];
u8 inb(u16 port);
#pragma aux inb = "in al,dx" parm [dx] value [al] modify exact [al];
u64 rdmsr(u32 msr);                                  /* edx:eax */
#pragma aux rdmsr = "rdmsr" parm [ecx] value [edx eax] modify exact [edx eax];
void wrmsr_(u32 msr, u32 lo, u32 hi);
#pragma aux wrmsr_ = "wrmsr" parm [ecx] [eax] [edx] modify exact [];
/* no C library in a VxD: take the halves of a 64-bit value without a shift helper */
static u32 hi32(u64 v) { union { u64 q; u32 d[2]; } u; u.q = v; return u.d[1]; }
static void wrmsr(u32 msr, u64 v) { wrmsr_(msr, (u32)v, hi32(v)); }

#pragma pack(push, 1)
struct idtr { u16 limit; u32 base; };
#pragma pack(pop)
void sidt(struct idtr *p);
#pragma aux sidt = "sidt fword ptr [eax]" parm [eax] modify exact [];
u16 get_cs(void);
#pragma aux get_cs = "mov ax,cs" value [ax] modify exact [ax];

/* 32x32 -> 64 multiply and 64/32 divide pieces, so that no 64-bit helper from the C library is needed */
u32 mulhi(u32 a, u32 b);
#pragma aux mulhi = "mul edx" parm [eax] [edx] value [edx] modify exact [eax edx];
u32 mul_shr16(u32 a, u32 b);
#pragma aux mul_shr16 = "mul edx" "shrd eax,edx,16" parm [eax] [edx] value [eax] modify exact [eax edx];
u32 divrem(u32 hi, u32 lo, u32 d);                   /* remainder of hi:lo / d, hi < d */
#pragma aux divrem = "div ecx" parm [edx] [eax] [ecx] value [edx] modify exact [eax edx];

u32 cpuid1_ecx(void);
#pragma aux cpuid1_ecx = "mov eax,1" "cpuid" value [ecx] modify exact [eax ebx ecx edx];

u32 get_cr2(void);
#pragma aux get_cr2 = "mov eax,cr2" value [eax] modify exact [eax];
u32 get_cr3(void);
#pragma aux get_cr3 = "mov eax,cr3" value [eax] modify exact [eax];

#endif
