/* gen2leg - legacy PC devices for Windows 9x on Hyper-V Generation 2 VMs (a VxD).
 *
 * Generation 2 VMs have no 8259 PICs, no 8254 PIT and nothing at port 61h, and Windows 9x's VPICD and VTD
 * (and other VxDs) program exactly these. The port instructions in a copy of VPICD and VTD were replaced
 * in place by `int vv` (hvkit vxd patch-io); this VxD owns those vectors and emulates the devices:
 *
 *   - 8259 x2: masks, init sequence, EOI (an EOI ends the interrupt at the local APIC) and IRR/ISR reads
 *   - 8254 channel 0: counting from the Hyper-V reference time; programming it arms synthetic timer 0 in
 *     direct mode, which delivers IRQ 0's vector (VPICD's) straight to the CPU
 *   - 8254 channel 2 and port 61h, for the timing loops that poll OUT2 and the refresh toggle
 *
 * Debug output goes to COM1 (0x3f8), which the Gen2 VM exposes as a named pipe.
 */
#include "vxd.h"
#include "compat.h"

struct regs;
void __cdecl shim_int(struct regs *r);
void __cdecl irq0_hook(u32 *frame);
void __cdecl nmi_hook(u32 *f);
void __cdecl control_msg(u32 msg);
void VXD_control(void);
static void lapic_dump(const char *why);

/* The DDB must be the first thing in the (locked) code object, and carries the DDK macro's signatures in
 * DDB_Prev and DDB_Reserved1-3: VMM refuses a VxD without them. */
DDB VXD_DDB = {
    NULL,                       /* must be NULL */
    DDK_VERSION,
    0,                          /* no device ID */
    1, 0,                       /* version 1.0 */
    NULL,
    { 'G', 'E', 'N', '2', 'L', 'E', 'G', ' ' },
    0x04000000,                 /* init order: before VPICD (0x0c000000) and VTD (0x14000000) */
    (DWORD)VXD_control,
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
    'Prev',
    sizeof(DDB),
    'Rsv1', 'Rsv2', 'Rsv3',
};

/* ---- operations the patched code asks for ----
 * Each (port, direction) the patched VPICD/VTD use has its own IDT vector, taken from vectors.txt (the
 * vectors that are free in VMM's 96-entry IDT); the stub of operation i pushes i. The last three stand for
 * `in al,dx ; ret`, `out dx,al ; ret` and `out dx,al ; jmp $+2 ; in al,dx` where DX is 20h/A0h/40h-43h. */
#define NVEC            22
struct op { u16 port; u8 out; u8 kind; };
enum { OP_PLAIN, OP_IN_DX_RET, OP_OUT_DX_RET, OP_OUT_IN_DX, OP_IN_DX, OP_OUT_DX };
static const struct op ops[NVEC] = {
    { 0x20, 0, 0 }, { 0x20, 1, 0 }, { 0x21, 0, 0 }, { 0x21, 1, 0 },
    { 0xa0, 0, 0 }, { 0xa0, 1, 0 }, { 0xa1, 0, 0 }, { 0xa1, 1, 0 },
    { 0x40, 0, 0 }, { 0x40, 1, 0 }, { 0x43, 1, 0 }, { 0x61, 0, 0 }, { 0x61, 1, 0 },
    { 0, 0, OP_IN_DX_RET }, { 0, 1, OP_OUT_DX_RET }, { 0, 1, OP_OUT_IN_DX },
    { 0x60, 0, 0 }, { 0x60, 1, 0 }, { 0x64, 0, 0 }, { 0x64, 1, 0 },     /* VKD's 8042 */
    /* Generic in/out with the port in DX, returning to the next instruction. SYSDETMG.DLL's port helper
     * (patched by hvkit vxd patch-sysdetmg) needs these: its port is a runtime argument, so no fixed-port op
     * matches, and unlike OP_IN_DX_RET it must fall through instead of returning. */
    { 0, 0, OP_IN_DX }, { 0, 1, OP_OUT_DX },
};
#include "vectors.h"            /* static const u8 vec_table[NVEC], from vectors.txt */

/* ---- hardware access: see compat.h ---- */

#define HV_MSR_TIME_REF         0x40000020      /* 100 ns units */
#define HV_MSR_EOI              0x40000070
#define HV_MSR_STIMER0_CONFIG   0x400000b0
#define HV_MSR_STIMER0_COUNT    0x400000b1
#define STIMER_CONFIG(vec, periodic) (1u | ((u32)(periodic) << 1) | (1u << 3) | ((u32)(vec) << 4) | (1u << 12))
/* The message SINT is unmasked only while messages are exchanged, AutoEOI, on a vector whose gate just irets
 * (messages are polled). It has to be a gate of VMM's 96-entry IDT and at least 16; 0x22 is one of VMM's DPL 0
 * default gates, which programs in VMs cannot reach. */
#define VMBUS_SINT_VECTOR       0x22

/* The PIT clock is 1193182 Hz, the reference time 10 MHz: ticks = ref * PIT_K >> 32. */
#define PIT_K           0x1e8ba336u     /* 1193182 * 2^32 / 10^7 */
/* 100 ns per PIT tick = 549254 / 65536 */
#define PIT_PERIOD_Q16  549254u

/* ---- COM1 log ---- */
#ifdef NOLOG
#define log(...) ((void)0)
#define com_putc(c) ((void)0)
#else
static u32 log_budget = 6000;
static void com_putc(char c)
{
    u32 n = 200000;
    while (!(inb(0x3fd) & 0x20) && --n)
        ;
    outb(0x3f8, c);
}
static void com_puts(const char *s) { while (*s) { if (*s == '\n') com_putc('\r'); com_putc(*s++); } }
static void com_hex(u32 v, int digits)
{
    for (int i = digits - 1; i >= 0; i--)
        com_putc("0123456789abcdef"[(v >> (4 * i)) & 15]);
}
/* %x (minimal digits, at least 1), %Nx is not supported: %2x/%4x/%8x give exactly that many digits */
static void log(const char *f, ...)
{
    if (!log_budget)
        return;
    log_budget--;
    va_list ap;
    va_start(ap, f);
    for (; *f; f++) {
        if (*f != '%') { if (*f == '\n') com_putc('\r'); com_putc(*f); continue; }
        f++;
        if (*f == '2' || *f == '4' || *f == '8') { int d = *f - '0'; com_hex(va_arg(ap, u32), d); }
        else if (*f == 's') com_puts(va_arg(ap, const char *));
        else if (*f == 'x') {
            u32 v = va_arg(ap, u32); int d = 1;
            for (u32 t = v >> 4; t; t >>= 4) d++;
            com_hex(v, d);
        } else if (*f == '%') com_putc('%');
    }
    va_end(ap);
}

#endif
/* ---- time ---- */
static u64 ref_now(void) { return rdmsr(HV_MSR_TIME_REF); }
/* a % b for a 64-bit a: (hi % b : lo) % b with one div */
static u32 mod64(u64 a, u32 b) { return divrem(hi32(a) % b, (u32)a, b); }
static u32 pit_ticks_since(u64 start)
{
    u64 d = ref_now() - start;
    return hi32(d) * PIT_K + mulhi((u32)d, PIT_K);
}

/* ---- 8259 ---- */
struct pic { u8 imr, icw_step, icw4, base, read_isr, isr, irr; };
static struct pic pic[2];
static u32 cascade_eois;        /* slave EOIs whose master (IRQ 2) EOI is still to come */
static u32 eoi_count;

static void hv_eoi(void) { wrmsr(HV_MSR_EOI, 0); }
static void timer_irq_mask_changed(void);
static void irq_unmasked(int n, u8 old, u8 now);
static void raise_irq(int irq);

static void pic_eoi(int n, int specific, int level)
{
    eoi_count++;
    if ((eoi_count & 4095) == 0 || eoi_count <= 4)
        log("gen2leg: EOI #%x (pic %x%s level %x) at ref %8, IMR %2/%2\n", eoi_count, n, specific ? " specific" : "", level,
            (u32)ref_now(), pic[0].imr, pic[1].imr);
    if (n == 0) {
        if (specific && level == 2)
            return;                     /* the cascade input: the slave's EOI already ended the interrupt */
        if (!specific && cascade_eois) {
            cascade_eois--;
            return;
        }
    } else {
        cascade_eois++;
    }
    hv_eoi();
}

static u8 pic_read(int n, int data)
{
    struct pic *p = &pic[n];
    if (data)
        return p->imr;
    return p->read_isr ? p->isr : p->irr;
}

static void pic_write(int n, int data, u8 v)
{
    struct pic *p = &pic[n];
    if (!data) {
        if (v & 0x10) {                 /* ICW1 */
            p->icw_step = 2; p->icw4 = v & 1; p->imr = 0; p->isr = p->irr = 0; p->read_isr = 0;
        } else if (v & 0x08) {          /* OCW3 */
            if ((v & 3) == 2) p->read_isr = 0;
            else if ((v & 3) == 3) p->read_isr = 1;
        } else {                        /* OCW2 */
            u8 cmd = v >> 5;
            if (cmd == 1 || cmd == 5)
                pic_eoi(n, 0, 0);
            else if (cmd == 3 || cmd == 7)
                pic_eoi(n, 1, v & 7);
        }
    } else if (p->icw_step == 2) {
        p->base = v & 0xf8; p->icw_step = 3;
    } else if (p->icw_step == 3) {
        p->icw_step = p->icw4 ? 4 : 0;
    } else if (p->icw_step == 4) {
        p->icw_step = 0;
    } else {
        u8 old = p->imr;
        p->imr = v;
        static u32 imr_logs;
        if (old != v && imr_logs++ < 24)    /* a log line takes milliseconds (COM1 exits): never per tick */
            log("gen2leg: IMR%x %2 -> %2 (APIC_BASE %8 STIMER0 %8/%8 now %8)\n", n, old, v, (u32)rdmsr(0x1b), (u32)rdmsr(HV_MSR_STIMER0_CONFIG),
                (u32)rdmsr(HV_MSR_STIMER0_COUNT), (u32)ref_now());
        if (n == 0 && ((old ^ v) & 1))
            timer_irq_mask_changed();
        if (old & ~v)
            irq_unmasked(n, old, v);
    }
}

/* ---- 8254 ---- */
struct pit {
    u16 reload;
    u8 mode, access;
    u8 wphase, rphase, loaded;
    u8 latched, status_latched, status;
    u16 latch;
    u64 start;                          /* reference time when counting began */
};
static struct pit pit[3];
static u8 sysctl;                       /* port 61h bits 0-3 */

/* IRQ 0 is wired to synthetic timer 0 */
static u8 t0_mode;                      /* 0 stopped, 1 periodic, 2 one-shot */
static u32 t0_period;                   /* 100 ns units */
static u64 t0_expiry;                   /* reference time of a pending one-shot */
static u8 t0_armed;

static u32 irq0_vector(void) { return pic[0].base; }

/* IRQ 0's gate is wrapped once the timer is first armed, to count the ticks that reach VPICD. */
static void stimer_oneshot(u64 abs);
u32 irq0_orig, irq0_ticks;
void irq0_wrap(void);
static void vmb_poll(void);
#ifdef EXIT_TEST
/* Test of the way back to DOS: some time after Init_Complete, leave Windows through Fatal_Error_Handler. */
static u64 exit_at;
static u32 exit_event;
static void __declspec(naked) exit_event_thunk(void);
static u32 __cdecl sched_global(void (*cb)(void));
#endif
void __cdecl irq0_hook(u32 *frame)
{
    irq0_ticks++;
    vmb_poll();
#ifdef EXIT_TEST
    if (exit_at && !exit_event && ref_now() > exit_at)
        exit_event = sched_global(exit_event_thunk);
#endif
    if (irq0_ticks <= 16)
        log("gen2leg: tick #%x esp %8 eip %8 cs %4 efl %8\n", irq0_ticks, (u32)frame, frame[8], frame[9], frame[10]);
    if (t0_armed) {
        if (t0_mode == 1) {                 /* periodic: the next tick, one period after the last one was due */
            u64 now = ref_now();
            t0_expiry += t0_period;
            if (t0_expiry <= now)
                t0_expiry = now + t0_period;
            stimer_oneshot(t0_expiry);
        } else {
            t0_armed = 0;
        }
    }
}
static void wrap_irq0(void)
{
    static u8 done;
    if (done)
        return;
    done = 1;
    struct idtr r;
    sidt(&r);
    u8 *g = (u8 *)r.base + irq0_vector() * 8;
    irq0_orig = *(u16 *)g | ((u32)*(u16 *)(g + 6) << 16);
    u32 h = (u32)irq0_wrap;
    *(u16 *)g = (u16)h;
    *(u16 *)(g + 6) = (u16)(h >> 16);
    log("gen2leg: IRQ0 gate (vector %2): %8 -> %8\n", irq0_vector(), irq0_orig, h);
}

static void stimer_stop(void)
{
    wrmsr(HV_MSR_STIMER0_CONFIG, 0);
    wrmsr(HV_MSR_STIMER0_COUNT, 0);
    t0_armed = 0;
}
/* Synthetic timer 0 in direct mode is always one-shot here (a periodic direct-mode timer was seen not to
 * interrupt at all): a periodic PIT is a chain of one-shots, the next one armed by the IRQ 0 wrapper. */
static void stimer_oneshot(u64 abs)
{
    wrmsr(HV_MSR_STIMER0_CONFIG, STIMER_CONFIG(irq0_vector(), 0));
    wrmsr(HV_MSR_STIMER0_COUNT, abs);
}
static u8 exited;
static void stimer_arm(void)
{
    if (exited)
        return;                         /* back to DOS: the BIOS owns the timer again */
    if (pic[0].imr & 1)
        return;                         /* IRQ 0 masked: stays pending in t0_mode */
    if (!t0_mode)
        return;
    u64 now = ref_now();
    if (t0_mode == 1)
        t0_expiry = now + t0_period;
    else if (t0_expiry <= now)
        t0_expiry = now + 1;
    stimer_oneshot(t0_expiry);
    t0_armed = 1;
    wrap_irq0();
    static u32 n;
    if (n++ < 6)
        log("gen2leg: STIMER0 now %8/%8 (%s, vector %2)\n", (u32)rdmsr(HV_MSR_STIMER0_CONFIG), (u32)rdmsr(HV_MSR_STIMER0_COUNT),
            t0_mode == 1 ? "periodic" : "one-shot", irq0_vector());
}
static void timer_irq_mask_changed(void)
{
    if (pic[0].imr & 1) {
        if (t0_armed)
            stimer_stop();
    } else if (t0_mode) {
        stimer_arm();
    }
}

static u32 reload_of(struct pit *p) { return p->reload ? p->reload : 0x10000; }

static u16 pit_count(struct pit *p)
{
    u64 el = pit_ticks_since(p->start);
    u32 r = reload_of(p);
    switch (p->mode) {
    case 0: case 1: case 4: case 5:
        if (el < r)
            return r - (u32)el;
        return (u16)(0x10000 - (u32)((el - r) & 0xffff));
    default:
        return r - mod64(el, r);
    }
}

static int pit_out(struct pit *p, int gate)
{
    if (!p->loaded)
        return 1;
    u64 el = pit_ticks_since(p->start);
    u32 r = reload_of(p);
    switch (p->mode) {
    case 0: case 1: case 5:
        return el >= r;
    case 2:
        return mod64(el, r) != r - 1;
    case 3:
        return mod64(el, r) < (r + 1) / 2;
    default:
        return el != r;
    }
    (void)gate;
}

static void pit_loaded(int ch)
{
    struct pit *p = &pit[ch];
    p->loaded = 1;
    p->start = ref_now();
    if (ch != 0)
        return;
    u32 r = reload_of(p);
    u32 period = mul_shr16(r, PIT_PERIOD_Q16);
    if (period < 1)
        period = 1;
    if (p->mode == 2 || p->mode == 3) {
        t0_mode = 1;
        t0_period = period;
    } else {
        t0_mode = 2;
        t0_expiry = p->start + period;
    }
    stimer_arm();
    static u32 n;
    if (n++ < 12)
        log("gen2leg: PIT0 mode %x reload %x -> %s %x (x100ns)%s\n", p->mode, p->reload,
            t0_mode == 1 ? "periodic" : "one-shot", period, (pic[0].imr & 1) ? " [IRQ0 masked]" : "");
}

static void pit_latch(struct pit *p)
{
    if (!p->latched) {
        p->latch = pit_count(p);
        p->latched = 1;
        p->rphase = 0;
    }
}

static void pit_ctrl(u8 v)
{
    int sel = v >> 6;
    if (sel == 3) {                     /* read-back */
        for (int c = 0; c < 3; c++) {
            if (!(v & (2 << c)))
                continue;
            struct pit *p = &pit[c];
            if (!(v & 0x20))
                pit_latch(p);
            if (!(v & 0x10)) {
                p->status = (pit_out(p, 1) << 7) | (p->loaded ? 0 : 0x40) | (p->access << 4) | (p->mode << 1);
                p->status_latched = 1;
            }
        }
        return;
    }
    struct pit *p = &pit[sel];
    int acc = (v >> 4) & 3;
    if (acc == 0) {
        pit_latch(p);
        return;
    }
    p->access = acc;
    p->mode = (v >> 1) & 7;
    if (p->mode >= 6)
        p->mode -= 4;
    p->wphase = p->rphase = p->latched = p->loaded = 0;
    if (sel == 0) {
        t0_mode = 0;
        if (t0_armed)
            stimer_stop();
    }
}

static void pit_write(int ch, u8 v)
{
    struct pit *p = &pit[ch];
    int done = 0;
    if (p->access == 1) { p->reload = v; done = 1; }
    else if (p->access == 2) { p->reload = (u16)v << 8; done = 1; }
    else if (!p->wphase) { p->reload = (p->reload & 0xff00) | v; p->wphase = 1; }
    else { p->reload = (p->reload & 0x00ff) | ((u16)v << 8); p->wphase = 0; done = 1; }
    if (done)
        pit_loaded(ch);
}

static u8 pit_read(int ch)
{
    struct pit *p = &pit[ch];
    if (p->status_latched) {
        p->status_latched = 0;
        return p->status;
    }
    u16 val = p->latched ? p->latch : pit_count(p);
    u8 out;
    if (p->access == 1) { out = val; p->latched = 0; }
    else if (p->access == 2) { out = val >> 8; p->latched = 0; }
    else if (!p->rphase) { out = val; p->rphase = 1; }
    else { out = val >> 8; p->rphase = 0; p->latched = 0; }
    return out;
}

static u8 sysctl_read(void)
{
    u8 refresh = mod64(ref_now(), 300) >= 150;      /* toggles every 15 us */
    u8 out2 = pit_out(&pit[2], sysctl & 1);
    return (sysctl & 0x0f) | (refresh << 4) | (out2 << 5);
}
static void sysctl_write(u8 v)
{
    u8 old = sysctl;
    sysctl = v & 0x0f;
    if (!(old & 1) && (v & 1) && pit[2].loaded)
        pit[2].start = ref_now();
}

/* ---- IRQ lines the shim drives itself ---- */
/* An IRQ is raised as a self-IPI with the vector VPICD gave it (x2APIC mode: IA32_X2APIC_SELF_IPI); masked in the
 * emulated IMR it stays pending until it is unmasked. */
static u16 irq_pending;
static void send_irq(int irq)
{
    wrmsr(0x83f, pic[irq >> 3].base + (irq & 7));
}
static void raise_irq(int irq)
{
    if (pic[irq >> 3].imr & (1 << (irq & 7)))
        irq_pending |= 1 << irq;
    else
        send_irq(irq);
}
static void irq_unmasked(int n, u8 old, u8 now)
{
    u8 bits = old & ~now;
    for (int b = 0; b < 8; b++) {
        int irq = n * 8 + b;
        if ((bits & (1 << b)) && (irq_pending & (1 << irq))) {
            irq_pending &= ~(1 << irq);
            send_irq(irq);
        }
    }
}

/* ---- 8042 keyboard controller with an AT keyboard and a PS/2 mouse behind it ----
 * What Windows 9x's keyboard driver (VKD) and mouse drivers expect to find at ports 60h/64h. Bytes for the guest
 * wait in a FIFO; IRQ 1 (keyboard) or IRQ 12 (mouse) is raised for each. kb_push()/mouse_push() are where a
 * synthetic keyboard/mouse (VMBus) will feed scancodes/packets. */
static u32 kbd_keys;                   /* keys from VMBus so far */
#define KB_FIFO 32
static u8 kb_cmd = 0x45;               /* controller command byte: bit0 kbd IRQ, bit1 aux IRQ, bit2 system flag, bit6 translate */
static u8 kb_outport = 0xcf;
static u8 kb_pend;                     /* data byte that a pending controller command (60h, d1h-d4h) takes */
static u8 kb_dev_arg;                  /* keyboard command waiting for its argument (ed, f0, f3) */
static u8 kb_mouse_arg;
static u8 kb_fifo[KB_FIFO], kb_aux[KB_FIFO];
static u8 kb_head, kb_tail;
static u8 kb_last_was_cmd;
static u32 kb_stats[4];

static void kb_irq(void)
{
    if (kb_head == kb_tail)
        return;
    if (kb_aux[kb_head] ? (kb_cmd & 2) : (kb_cmd & 1))
        raise_irq(kb_aux[kb_head] ? 12 : 1);
}
static void kb_push_src(u8 v, u8 aux)
{
    u8 n = (kb_tail + 1) % KB_FIFO;
    if (n == kb_head)
        return;
    int was_empty = kb_head == kb_tail;
    kb_fifo[kb_tail] = v;
    kb_aux[kb_tail] = aux;
    kb_tail = n;
    if (was_empty)
        kb_irq();
}
static void kb_push(u8 v) { kb_push_src(v, 0); }
static void mouse_push(u8 v) { kb_push_src(v, 1); }

static u8 kb_read_status(void)
{
    u8 st = 0x10;                       /* keyboard not inhibited */
    if (kb_head != kb_tail)
        st |= 1 | (kb_aux[kb_head] ? 0x20 : 0);
    if (kb_cmd & 4)
        st |= 4;                        /* system flag */
    if (kb_last_was_cmd)
        st |= 8;
    return st;
}
static u8 kb_read_data(void)
{
    if (kb_head == kb_tail)
        return 0;
    u8 v = kb_fifo[kb_head];
    kb_head = (kb_head + 1) % KB_FIFO;
    static u32 n;
    if (kbd_keys && n++ < 24)
        log("gen2leg: port 60h -> %2\n", v);
    kb_irq();                           /* the next byte, if any */
    return v;
}
/* Pulsing the 8042's reset line (command 0xfe) is how Windows restarts the machine. Gen2 has no 8042, so
 * reset the CPU the other way an 8042 reset was commonly done: a triple fault (empty IDT, then an exception).
 * Hyper-V resets the VM on a triple fault. Windows has flushed and shut down by the time it sends this.
 * TODO: a clean reset through the BIOS (UEFI ResetSystem on the BIOS helper core) once SeaBIOS publishes an
 * entry point for it. */
static void system_reset(void)
{
    static struct idtr none;
    log("gen2leg: 8042 reset line pulsed: resetting the VM (triple fault)\n");
    _asm {
        cli
        lidt fword ptr [none]
        int 3
    }
}
static void kb_write_cmd(u8 v)
{
    static u32 n;
    kb_last_was_cmd = 1;
    kb_pend = 0;
    if (n++ < 40)
        log("gen2leg: 8042 command %2\n", v);
    switch (v) {
    case 0x20: kb_push(kb_cmd); break;
    case 0x60: case 0xd1: case 0xd2: case 0xd3: case 0xd4: kb_pend = v; break;
    case 0xaa: kb_cmd |= 4; kb_head = kb_tail = 0; kb_push(0x55); break;       /* self test */
    case 0xab: kb_push(0x00); break;                                          /* keyboard interface test */
    case 0xa9: kb_push(0x00); break;                                          /* mouse interface test */
    case 0xad: kb_cmd |= 0x10; break;
    case 0xae: kb_cmd &= ~0x10; break;
    case 0xa7: kb_cmd |= 0x20; break;
    case 0xa8: kb_cmd &= ~0x20; break;
    case 0xd0: kb_push(kb_outport); break;
    case 0xc0: kb_push(0x00); break;
    case 0xfe: system_reset(); break;
    default: break;
    }
}
static void kb_device_write(u8 v)
{
    if (kb_dev_arg) {                   /* argument of ed (LEDs), f0 (scancode set), f3 (typematic) */
        kb_dev_arg = 0;
        kb_push(0xfa);
        return;
    }
    switch (v) {
    case 0xff: kb_push(0xfa); kb_push(0xaa); break;                           /* reset, self test passed */
    case 0xf2: kb_push(0xfa); kb_push(0xab); kb_push(0x83); break;           /* identify: MF2 keyboard */
    case 0xee: kb_push(0xee); break;                                          /* echo */
    case 0xed: case 0xf0: case 0xf3: kb_dev_arg = v; kb_push(0xfa); break;
    default: kb_push(0xfa); break;
    }
}
static void kb_mouse_write(u8 v)
{
    if (kb_mouse_arg) {                 /* argument of f3 (sample rate), e8 (resolution) */
        kb_mouse_arg = 0;
        mouse_push(0xfa);
        return;
    }
    switch (v) {
    case 0xff: mouse_push(0xfa); mouse_push(0xaa); mouse_push(0x00); break;   /* reset: ACK, self test, device id */
    case 0xf2: mouse_push(0xfa); mouse_push(0x00); break;                     /* identify: standard PS/2 mouse */
    case 0xe9: mouse_push(0xfa); mouse_push(0x00); mouse_push(0x02); mouse_push(0x64); break;   /* status */
    case 0xf3: case 0xe8: kb_mouse_arg = v; mouse_push(0xfa); break;
    default: mouse_push(0xfa); break;
    }
}
static void kb_write_data(u8 v)
{
    kb_last_was_cmd = 0;
    u8 p = kb_pend;
    kb_pend = 0;
    switch (p) {
    case 0x60: kb_cmd = v; return;
    case 0xd1: kb_outport = v; return;
    case 0xd2: kb_push(v); return;
    case 0xd3: mouse_push(v); return;
    case 0xd4: kb_mouse_write(v); return;
    default: break;
    }
    kb_device_write(v);
}

/* ---- port dispatch ---- */
static int emulated(u16 port)
{
    return port == 0x20 || port == 0x21 || port == 0xa0 || port == 0xa1 ||
           (port >= 0x40 && port <= 0x43) || port == 0x60 || port == 0x61 || port == 0x64;
}
static u8 io_read(u16 port)
{
    switch (port) {
    case 0x20: return pic_read(0, 0);
    case 0x21: return pic_read(0, 1);
    case 0xa0: return pic_read(1, 0);
    case 0xa1: return pic_read(1, 1);
    case 0x40: case 0x41: case 0x42: return pit_read(port - 0x40);
    case 0x43: return 0xff;
    case 0x60: return kb_read_data();
    case 0x61: return sysctl_read();
    case 0x64: return kb_read_status();
    }
    return 0xff;
}
static void io_write(u16 port, u8 v)
{
    switch (port) {
    case 0x20: pic_write(0, 0, v); break;
    case 0x21: pic_write(0, 1, v); break;
    case 0xa0: pic_write(1, 0, v); break;
    case 0xa1: pic_write(1, 1, v); break;
    case 0x40: case 0x41: case 0x42: pit_write(port - 0x40, v); break;
    case 0x43: pit_ctrl(v); break;
    case 0x60: kb_write_data(v); break;
    case 0x61: sysctl_write(v); break;
    case 0x64: kb_write_cmd(v); break;
    }
}
/* ports from DX at run time: the emulated ones, anything else for real */
static u8 dx_in(u16 port) { return emulated(port) ? io_read(port) : inb(port); }
static void dx_out(u16 port, u8 v) { if (emulated(port)) io_write(port, v); else outb(port, v); }

/* ---- interrupt entry ---- */
struct regs { u32 edi, esi, ebp, esp0, ebx, edx, ecx, eax, op, eip, cs, eflags; };
void ret_thunk(void);
static u32 seen[NVEC];
static void hook_nmi(void);

void __cdecl shim_int(struct regs *r)
{
    u32 i = r->op;
    if (i >= NVEC)
        return;
    const struct op *o = &ops[i];
    {
        static u32 calls;
        if (calls++ < 12)
            lapic_dump("op");
    }
    if (seen[i]++ < 2)
        log("gen2leg: op %x (vector %2) first used at %8 (edx %x eax %x)\n", i, vec_table[i], r->eip, r->edx, r->eax);
    if (o->kind == OP_PLAIN) {
        if (o->out)
            io_write(o->port, (u8)r->eax);
        else
            *(u8 *)&r->eax = io_read(o->port);
        return;
    }
    u16 port = (u16)r->edx;
    switch (o->kind) {
    case OP_IN_DX_RET:
        *(u8 *)&r->eax = dx_in(port);
        r->eip = (u32)ret_thunk;
        break;
    case OP_OUT_DX_RET:
        dx_out(port, (u8)r->eax);
        r->eip = (u32)ret_thunk;
        break;
    case OP_OUT_IN_DX:
        dx_out(port, (u8)r->eax);
        *(u8 *)&r->eax = dx_in(port);
        break;
    case OP_IN_DX:
        /* zero-extended: the patched site's `xor ah,ah` is a nop now */
        r->eax = dx_in(port);
        break;
    case OP_OUT_DX:
        dx_out(port, (u8)r->eax);
        break;
    }
}

/* ---- local APIC, mapped through VMM to see what is pending/in service ---- */
ULONG __declspec(naked) __cdecl _MapPhysToLinear(ULONG PhysAddr, ULONG nBytes, ULONG flags)
{
    VMMJmp(_MapPhysToLinear);
}
static void lapic_dump(const char *why)
{
    static u32 n;
    if (!(rdmsr(0x1b) & 0x400) || n++ >= 90)
        return;
    struct idtr r;
    sidt(&r);
    u8 *g = (u8 *)r.base + 0x50 * 8;
    log("gen2leg: LAPIC [%s] id %8 TPR %2 PPR %2 SVR %8 LVT timer %8 LINT0 %8 LINT1 %8 | gate50 %8 (ours %8)\n", why,
        (u32)rdmsr(0x802), (u32)rdmsr(0x808), (u32)rdmsr(0x80a), (u32)rdmsr(0x80f), (u32)rdmsr(0x832), (u32)rdmsr(0x835),
        (u32)rdmsr(0x836), *(u16 *)g | ((u32)*(u16 *)(g + 6) << 16), (u32)irq0_wrap);
    log("  ISR %8 %8 %8 %8 %8 %8 %8 %8\n", (u32)rdmsr(0x810), (u32)rdmsr(0x811), (u32)rdmsr(0x812), (u32)rdmsr(0x813),
        (u32)rdmsr(0x814), (u32)rdmsr(0x815), (u32)rdmsr(0x816), (u32)rdmsr(0x817));
    log("  IRR %8 %8 %8 %8 %8 %8 %8 %8\n", (u32)rdmsr(0x820), (u32)rdmsr(0x821), (u32)rdmsr(0x822), (u32)rdmsr(0x823),
        (u32)rdmsr(0x824), (u32)rdmsr(0x825), (u32)rdmsr(0x826), (u32)rdmsr(0x827));
}

/* ---- NMI sampling ---- */
u32 nmi_orig;
void nmi_wrap(void);
void __cdecl nmi_hook(u32 *f)
{
    static u32 n;
    u32 eip = f[0], cs = f[1], efl = f[2];
    log("gen2leg: NMI #%x at %4:%8 eflags %8 (esp %8) irq0 ticks %x, EOIs %x\n", ++n, cs, eip, efl, (u32)f, irq0_ticks, eoi_count);
    log("  stack:");
    for (int i = 3; i < 11; i++)
        log(" %8", f[i]);
    log("\n");
    if (eip >= 0xc0000000u) {                       /* kernel space: show the code around eip */
        log("  code @eip-8:");
        for (int i = -8; i < 24; i++)
            log(" %2", ((u8 *)eip)[i]);
        log("\n");
    }
}
/* Wrap an IDT gate: ours runs first, then VMM's handler (whatever is in the gate when we look). VMM sets
 * its own NMI gate after our Sys_Critical_Init, so the wrappers are re-checked whenever we are called. */
static void wrap_gate(u32 vec, void (*stub)(void), u32 *orig)
{
    struct idtr r;
    sidt(&r);
    u8 *g = (u8 *)r.base + vec * 8;
    u32 cur = *(u16 *)g | ((u32)*(u16 *)(g + 6) << 16);
    u32 h = (u32)stub;
    if (cur == h)
        return;
    *orig = cur;
    *(u16 *)g = (u16)h;
    *(u16 *)(g + 6) = (u16)(h >> 16);
    log("gen2leg: gate %2: %8 -> %8\n", vec, cur, h);
}

/* Exceptions: report the interesting ones, then continue at VMM's handler. */
u32 eo0, eo6, eo10, eo11, eo13, eo17;
void __cdecl exc_hook(u32 vec, u32 *f, u32 has_err)
{
    static u32 n, pf;
    if (vec == 14 && pf++ >= 8)
        return;
    if (n++ >= 60)
        return;
    u32 err = has_err ? f[0] : 0;
    u32 *fr = has_err ? f + 1 : f;
    log("gen2leg: EXC %2 err %8 at %4:%8 eflags %8 cr2 %8 cr3 %8 esp %8\n", vec, err, fr[1], fr[0], fr[2], get_cr2(), get_cr3(), (u32)f);
    log("  stack:");
    for (int i = 3; i < 11; i++)
        log(" %8", fr[i]);
    log("\n");
    if (fr[0] >= 0xc0000000u && !(fr[2] & 0x20000)) {
        log("  code @eip-8:");
        for (int i = -8; i < 16; i++)
            log(" %2", ((u8 *)fr[0])[i]);
        log("\n");
    }
}
/* Write the frame to COM1 using only registers and the (small) stack in place: no globals, no calls.
 * Output: "X<letter> <8 dwords of the frame as seen above pushad>" and continue at VMM's handler. */
static void __declspec(naked) exc_dump(void);
#define EXC_STUB(v, err) static void __declspec(naked) exc##v(void) { \
    _asm pushad \
    _asm mov bl, 'A'+v \
    _asm mov bh, 8 + 4 * err \
    _asm call exc_dump \
    _asm popad \
    _asm jmp dword ptr cs:[eo##v] }
u32 exc_budget = 64;                    /* each line costs milliseconds, and Win16 code faults all the time */
static void __declspec(naked) exc_dump(void)
{
    /* called with the pushad frame at esp+4; bl = the letter */
    _asm {
        movzx ecx, bh                   ; where eflags is in the frame: 8 (no error code) or 12
        test dword ptr [esp + ecx + 36], 0x20000
        jnz done                        ; V86 frames are routine (VMM emulates them): not shown
        cmp dword ptr ss:[exc_budget], 0
        je done
        dec dword ptr ss:[exc_budget]
        mov dx, 0x3f8
        mov al, 'X'
        out dx, al
        mov al, bl
        out dx, al
        mov esi, 36                     ; skip our return address and the 8 pushad dwords
        mov ecx, 8
    next_word:
        mov al, ' '
        out dx, al
        mov edi, [esp + esi]
        mov ebp, 8
    next_digit:
        rol edi, 4
        mov eax, edi
        and al, 15
        add al, '0'
        cmp al, '9'
        jbe is_digit
        add al, 7
    is_digit:
        out dx, al
        dec ebp
        jnz next_digit
        add esi, 4
        dec ecx
        jnz next_word
        mov al, 13
        out dx, al
        mov al, 10
        out dx, al
    done:
        ret
    }
}
/* not #PF, #SS or #DF: VMM maps stack pages on demand, and a wrapper that pushes onto a stack page that is not
 * there yet faults again inside the fault */
EXC_STUB(0, 0) EXC_STUB(6, 0) EXC_STUB(10, 1) EXC_STUB(11, 1) EXC_STUB(13, 1) EXC_STUB(17, 1)
static void hook_nmi(void)
{
    wrap_gate(2, nmi_wrap, &nmi_orig);
    wrap_gate(0, exc0, &eo0);
    wrap_gate(6, exc6, &eo6);
    wrap_gate(10, exc10, &eo10);
    wrap_gate(11, exc11, &eo11);
    wrap_gate(13, exc13, &eo13);
    wrap_gate(17, exc17, &eo17);
}

/* ---- VxD ---- */
extern void (*const stubs[NVEC])(void);
static void __declspec(naked) sint_iret(void) { _asm iretd }

#ifdef PROBE_IDT
/* Report the IDT: its limit, and the vectors whose gate is not present (the only ones usable). */
static void install_gates(void)
{
    struct idtr r;
    u16 cs;
    sidt(&r);
    cs = get_cs();
    log("gen2leg: IDT at %8 limit %x, cs %x\n", r.base, r.limit, cs);
    log("gen2leg: free vectors:");
    for (u32 v = 0; v <= r.limit / 8; v++) {
        u8 *g = (u8 *)r.base + v * 8;
        if (!(g[5] & 0x80))
            log(" %2", v);
    }
    log("\n");
    for (u32 v = 0; v <= r.limit / 8; v++) {
        u8 *g = (u8 *)r.base + v * 8;
        u32 h = *(u16 *)g | ((u32)*(u16 *)(g + 6) << 16);
        log("%2:%2:%8 ", v, g[5], h);
        if ((v & 3) == 3)
            log("\n");
    }
}
#else
static void install_gates(void)
{
    struct idtr r;
    u16 cs;
    sidt(&r);
    cs = get_cs();
    log("gen2leg: IDT at %8 limit %x, cs %x\n", r.base, r.limit, cs);
    for (u32 i = 0; i < NVEC; i++) {
        u32 v = vec_table[i];
        if (v > r.limit / 8) {
            log("gen2leg: vector %2 is beyond the IDT\n", v);
            continue;
        }
        u8 *g = (u8 *)r.base + v * 8;
        log("gen2leg: vector %2 was %8 (type %2)\n", v, *(u16 *)g | ((u32)*(u16 *)(g + 6) << 16), g[5]);
        u32 h = (u32)stubs[i];
        *(u16 *)g = (u16)h;
        *(u16 *)(g + 2) = cs;
        g[4] = 0;
        g[5] = 0x8e;                    /* present, DPL 0, 32-bit interrupt gate */
        *(u16 *)(g + 6) = (u16)(h >> 16);
    }
    u8 *g = (u8 *)r.base + VMBUS_SINT_VECTOR * 8;
    u32 h = (u32)sint_iret;
    log("gen2leg: VMBus SINT vector %2 was %8 (type %2)\n", VMBUS_SINT_VECTOR, *(u16 *)g | ((u32)*(u16 *)(g + 6) << 16), g[5]);
    *(u16 *)g = (u16)h;
    *(u16 *)(g + 2) = cs;
    g[4] = 0;
    g[5] = 0x8e;
    *(u16 *)(g + 6) = (u16)(h >> 16);
}
#endif

/* ---- VMBus input: keyboard and mouse on the BIOS's connection (../vmbc) ----
 * SeaBIOS made the VMBus connection, opened the keyboard channel (which it reads from its timer tick in DOS) and
 * published both in its handoff block. While Windows runs the BIOS tick is off, so the IRQ 0 hook reads the keyboard
 * ring instead and feeds the scancodes to the 8042 model. The mouse channel is opened at Device_Init, read from the
 * same hook and posted to VMOUSE as absolute positions, and closed again at System_Exit, so that DOS gets the
 * connection back the way the BIOS left it. Channel messages are only exchanged at those two points, synchronously
 * with interrupts off (Device_Init stacks are too small for ticks nesting into a wait). */
ULONG __declspec(naked) __cdecl _CopyPageTable(ULONG LinPgNum, ULONG nPages, DWORD *PageBuf, ULONG flags)
{
    VMMJmp(_CopyPageTable);
}
#define HV_MSR_HYPERCALL        0x40000001

void vmbc_mb(void);
#pragma aux vmbc_mb = "lock or dword ptr [esp],0" modify exact [];
#define VMBC_HAVE_TYPES
#include "vmbc.c"

/* Hyper-V hypercall, 32-bit convention: control edx:eax, input GPA (or fast input) ebx:ecx, output GPA edi:esi. */
static u32 hc_page_ptr;
static u32 __declspec(naked) __cdecl hc_call(u32 control, u32 input, u32 output)
{
    _asm {
        push ebx
        push esi
        push edi
        mov eax, [esp + 16]
        mov ecx, [esp + 20]
        mov esi, [esp + 24]
        xor edx, edx
        xor ebx, ebx
        xor edi, edi
        call dword ptr [hc_page_ptr]
        pop edi
        pop esi
        pop ebx
        ret
    }
}
u32 vmbc_rdmsr(u32 msr, u32 *hi)
{
    u64 v = rdmsr(msr);
    if (hi)
        *hi = hi32(v);
    return (u32)v;
}
void vmbc_wrmsr(u32 msr, u32 lo, u32 hi) { wrmsr_(msr, lo, hi); }
u32 vmbc_hypercall(u32 control, u32 input, u32 output) { return hc_call(control, input, output); }
void *vmbc_map(u32 phys, u32 len)
{
    u32 lin = _MapPhysToLinear(phys, len ? len : 1, 0);
    return lin == 0xffffffffu ? 0 : (void *)lin;
}
u32 vmbc_phys(void *p)
{
    DWORD pte = 0;
    _CopyPageTable((u32)p >> 12, 1, &pte, 0);
    return ((u32)pte & 0xfffff000u) | ((u32)p & 0xfff);
}

u32 cli_save(void);
#pragma aux cli_save = "pushfd" "pop eax" "cli" value [eax] modify exact [eax];
void flags_restore(u32 f);
#pragma aux flags_restore = "push eax" "popfd" parm [eax] modify exact [];

/* VMOUSE */
#define VMD__Get_Version        0
#define VMD__Post_Absolute_Pointer_Message 11
static u32 __declspec(naked) __cdecl get_ddb(u32 id)
{
    _asm {
        push ebx
        push ecx
        push edi
        mov eax, [esp + 16]
        xor edi, edi
    }
    VMMCall(Get_DDB)
    _asm {
        mov eax, ecx
        pop edi
        pop ecx
        pop ebx
        ret
    }
}
static u32 __declspec(naked) __cdecl vmd_version(void)
{
    _asm {
        push ebx
        push ecx
        push edx
        push esi
        push edi
        push ebp
    }
    VxDCall(VMD, Get_Version)
    _asm {
        pop ebp
        pop edi
        pop esi
        pop edx
        pop ecx
        pop ebx
        ret
    }
}
/* x, y: 0..65535 across the screen; buttons: VMOUSE's format, bit 5 left, bit 4 right, bit 3 middle */
static void __declspec(naked) __cdecl vmd_post_abs(u32 x, u32 y, u32 buttons)
{
    _asm {
        pushad
        mov esi, [esp + 36]
        mov edi, [esp + 40]
        mov eax, [esp + 44]
    }
    VxDCall(VMD, Post_Absolute_Pointer_Message)
    _asm {
        popad
        ret
    }
}
static u32 __declspec(naked) __cdecl sched_global(void (*cb)(void))
{
    _asm {
        push esi
        push edx
        mov esi, [esp + 12]
        xor edx, edx
    }
    VMMCall(Schedule_Global_Event)
    _asm {
        mov eax, esi
        pop edx
        pop esi
        ret
    }
}

static struct vmbc vc;
static struct vmbc_chan kbd, mouse;
static u8 kbd_ok, mouse_ok;
#define MOUSE_RING_PAGES        2
static u8 vmb_raw[4096 * (4 + 2 * MOUSE_RING_PAGES)];
static u8 *vmb_pages(void) { return (u8 *)(((u32)vmb_raw + 4095) & ~4095u); }  /* 3 message pages, then the rings */
static u32 vmb_msg[60];

/* Feed channel messages to ch until it reaches state want (or fails); interrupts are off. */
static int vmb_wait_state(struct vmbc_chan *ch, int want, u32 ms)
{
    u64 end = ref_now() + ms * 10000u;
    while (ch->state != want && ch->state != VMBC_FAILED) {
        int t = vmbc_poll_msg(&vc, vmb_msg);
        if (t >= 0 && !vmbc_chan_msg(&vc, ch, vmb_msg, t))
            log("gen2leg: VMBus message %x ignored\n", t);
        if (ref_now() > end)
            return -1;
    }
    return ch->state == want ? 0 : -1;
}
static void vmb_msg_begin(void)
{
    u32 *p = (u32 *)vmb_pages();
    for (int i = 0; i < 3 * 1024; i++)
        p[i] = 0;
    vmbc_msg_begin(&vc, p, VMBUS_SINT_VECTOR);
}

/* Synthetic HID (mouse): pipe header, then message header and data */
#define HID_PROTOCOL_REQUEST            0
#define HID_PROTOCOL_RESPONSE           1
#define HID_INITIAL_DEVICE_INFO         2
#define HID_INITIAL_DEVICE_INFO_ACK     3
#define HID_INPUT_REPORT                4
struct hid_msg { u32 pipe_type, pipe_size, type, size; u8 data[240]; };
static struct hid_msg hid_out, hid_in;

static int hid_send(u32 type, u32 data, u32 len, u32 id)
{
    hid_out.pipe_type = 1;
    hid_out.pipe_size = 8 + len;
    hid_out.type = type;
    hid_out.size = len;
    *(u32 *)hid_out.data = data;
    return vmbc_send(&vc, &mouse, VM_PKT_DATA_INBAND, VMBUS_DATA_PACKET_FLAG_COMPLETION_REQUESTED, &hid_out, 16 + len,
                     id);
}
/* Protocol 2.0 request, response, device info, ack; interrupts are off. */
static int hid_handshake(void)
{
    if (hid_send(HID_PROTOCOL_REQUEST, 0x00020000, 4, 1))
        return -1;
    u64 end = ref_now() + 10000000u;
    while (ref_now() < end) {
        u16 type;
        int len = vmbc_recv(&mouse, &hid_in, sizeof(hid_in), &type);
        if (len < 16 || type != VM_PKT_DATA_INBAND)
            continue;
        if (hid_in.type == HID_PROTOCOL_RESPONSE && !hid_in.data[4]) {
            log("gen2leg: mouse protocol %8 not approved\n", *(u32 *)hid_in.data);
            return -1;
        }
        if (hid_in.type == HID_INITIAL_DEVICE_INFO) {
            u16 *info = (u16 *)hid_in.data;         /* u32 size, u16 vendor, product, version */
            log("gen2leg: mouse %4:%4 version %4\n", info[2], info[3], info[4]);
            return hid_send(HID_INITIAL_DEVICE_INFO_ACK, 0, 1, 2);
        }
    }
    log("gen2leg: mouse handshake timed out\n");
    return -1;
}
static void mouse_close(void)
{
    if (mouse.state == VMBC_CLOSED)
        return;
    mouse_ok = 0;
    u32 fl = cli_save();
    u64 t0 = ref_now();
    vmb_msg_begin();
    int r = vmbc_close(&vc, &mouse);
    if (!r)
        r = vmb_wait_state(&mouse, VMBC_CLOSED, 500);
    vmbc_msg_end(&vc);
    flags_restore(fl);
    log("gen2leg: mouse channel closed: %s (state %x) in %x00 ns\n", r ? "FAILED" : "ok", mouse.state, (u32)(ref_now() - t0));
}
static void mouse_open(struct vmbus_handoff_offer *o)
{
    u8 *ring = vmb_pages() + 3 * 4096;
    for (int i = 0; i < 2 * MOUSE_RING_PAGES * 1024; i++)
        ((u32 *)ring)[i] = 0;
    u32 fl = cli_save();
    u64 t0 = ref_now();
    vmb_msg_begin();
    int r = vmbc_open(&vc, &mouse, o, ring, MOUSE_RING_PAGES);
    if (!r)
        r = vmb_wait_state(&mouse, VMBC_OPEN, 1000);
    vmbc_msg_end(&vc);
    if (!r)
        r = hid_handshake();
    flags_restore(fl);
    log("gen2leg: mouse channel (relid %x, gpadl %x): %s (state %x status %x) in %x00 ns\n", o->relid, mouse.gpadl,
        r ? "FAILED" : "open", mouse.state, mouse.status, (u32)(ref_now() - t0));
    if (r)
        mouse_close();
    else
        mouse_ok = 1;
}

/* Reports go to VMOUSE from a global event, not from the interrupt. Moves are merged, but every change of the
 * buttons is posted on its own, or a click within one tick would be lost. */
#define MOUSE_Q 16
static struct { u32 x, y, buttons; } mouse_q[MOUSE_Q];
static u32 mouse_head, mouse_tail, mouse_event, mouse_posts;   /* the event takes from head, the tick adds at tail */
static void __declspec(naked) mouse_event_thunk(void);
void __cdecl mouse_event_cb(void)
{
    static u32 vmd;
    if (!vmd) {
        vmd = get_ddb(VMD_DEVICE_ID) ? vmd_version() | 0x80000000u : 1;
        log("gen2leg: VMOUSE %s, version %4\n", vmd & 0x80000000u ? "present" : "missing", vmd & 0xffff);
    }
    for (;;) {
        u32 fl = cli_save();
        if (mouse_head == mouse_tail) {
            mouse_event = 0;
            flags_restore(fl);
            return;
        }
        u32 x = mouse_q[mouse_head].x, y = mouse_q[mouse_head].y, b = mouse_q[mouse_head].buttons;
        mouse_head = (mouse_head + 1) % MOUSE_Q;
        flags_restore(fl);
        if (!(vmd & 0x80000000u))
            continue;
        if (mouse_posts++ < 20)
            log("gen2leg: mouse %4,%4 buttons %2\n", x, y, b);
        vmd_post_abs(x, y, b);
    }
}
static void mouse_poll(void)
{
    u16 type;
    int n = 0;
    while (n++ < 32 && vmbc_recv(&mouse, &hid_in, sizeof(hid_in), &type) >= 0) {
        if (type != VM_PKT_DATA_INBAND || hid_in.type != HID_INPUT_REPORT || hid_in.size < 5)
            continue;
        /* buttons (bit 0 left, 1 right, 2 middle), X and Y 0..7fff, wheel */
        u8 *d = hid_in.data;
        u32 x = d[1] | (u32)d[2] << 8, y = d[3] | (u32)d[4] << 8;
        u32 b = (d[0] & 1 ? 0x20 : 0) | (d[0] & 2 ? 0x10 : 0) | (d[0] & 4 ? 0x08 : 0);
        u32 last = (mouse_tail + MOUSE_Q - 1) % MOUSE_Q;
        if (mouse_head == mouse_tail || mouse_q[last].buttons != b) {
            u32 next = (mouse_tail + 1) % MOUSE_Q;
            if (next == mouse_head)
                continue;                   /* full: VMOUSE is not taking them */
            last = mouse_tail;
            mouse_tail = next;
        }
        mouse_q[last].x = x * 2 + (x >> 14);
        mouse_q[last].y = y * 2 + (y >> 14);
        mouse_q[last].buttons = b;
    }
    if (mouse_head != mouse_tail && !mouse_event)
        mouse_event = sched_global(mouse_event_thunk);
}

/* SeaBIOS's synthetic keyboard ring: set 1 make/break codes, into the 8042 */
#define HK_MSG_EVENT                    3
#define HK_KEY_UNICODE                  1
#define HK_KEY_BREAK                    2
#define HK_KEY_E0                       4
#define HK_KEY_E1                       8
static void kbd_poll(int discard)
{
    struct { u32 type; u16 make_code, reserved; u32 info; } m;
    u16 type;
    int n = 0;
    while (n++ < 16 && vmbc_recv(&kbd, &m, sizeof(m), &type) >= 12) {
        if (type != VM_PKT_DATA_INBAND || m.type != HK_MSG_EVENT || (m.info & HK_KEY_UNICODE) || discard)
            continue;
        if (kbd_keys++ < 40)
            log("gen2leg: key %2%s%s\n", m.make_code, m.info & HK_KEY_E0 ? " e0" : "", m.info & HK_KEY_BREAK ? " up" : "");
        if (m.info & HK_KEY_E0)
            kb_push(0xe0);
        if (m.info & HK_KEY_E1)
            kb_push(0xe1);
        kb_push((u8)m.make_code | (m.info & HK_KEY_BREAK ? 0x80 : 0));
    }
}

/* From the IRQ 0 hook */
static void vmb_poll(void)
{
    if (kbd_ok)
        kbd_poll(0);
    if (mouse_ok)
        mouse_poll();
}

static void vmb_setup(void)
{
    if (vmbc_init(&vc)) {
        log("gen2leg: no VMBus handoff block from the BIOS\n");
        return;
    }
    u32 hc = (u32)rdmsr(HV_MSR_HYPERCALL);
    hc_page_ptr = hc & 1 ? (u32)vmbc_map(hc & ~0xfffu, 4096) : 0;
    struct vmbus_handoff *ho = vc.ho;
    log("gen2leg: VMBus handoff: version %8 conn %x sint %x vp %x, %x offers, next gpadl %8, hypercall page %8\n",
        ho->version, ho->msg_conn_id, ho->sint, ho->vp_index, ho->offer_count, ho->next_gpadl, hc_page_ptr);
    if (!hc_page_ptr)
        return;
    struct vmbus_handoff_offer *o = vmbc_offer(&vc, VMBC_GUID_KEYBOARD, 0);
    if (o && !vmbc_adopt(&vc, &kbd, o)) {
        kbd_poll(1);                    /* keys typed at the BIOS that it did not pick up */
        kbd_ok = 1;
        log("gen2leg: keyboard: the BIOS's channel, relid %x\n", kbd.relid);
    } else {
        log("gen2leg: no BIOS keyboard channel\n");
    }
    o = vmbc_offer(&vc, VMBC_GUID_MOUSE, 0);
    if (o)
        mouse_open(o);
    else
        log("gen2leg: no mouse offer\n");
}

/* ---- messages Windows shows in text mode ----
 * VDD draws system-modal messages ("message mode") straight into VGA text memory, which nothing shows on a
 * Gen2 VM while Windows runs (the BIOS scan-out runs from the BIOS tick). Copy them to COM1. */
#define SHELL__SYSMODAL_Message 3
#define SHELL__Message          4
static u32 __declspec(naked) __cdecl hook_service(u32 id, void (*proc)(void))
{
    _asm {
        push esi
        mov eax, [esp + 8]
        mov esi, [esp + 12]
    }
    VMMCall(Hook_Device_Service)
    _asm {
        mov eax, esi
        jnc hooked
        xor eax, eax
    hooked:
        pop esi
        ret
    }
}
void __cdecl shell_msg_log(u32 svc, const char *msg, const char *caption, u32 flags)
{
    static u32 n;
    if (n++ >= 20)
        return;
    log("gen2leg: SHELL %s flags %8 [%s]\n", svc == SHELL__SYSMODAL_Message ? "SYSMODAL_Message" : "Message", flags,
        caption ? caption : "");
    if (msg)
        log("  %s\n", msg);
}
u32 shell_prev_sysmodal, shell_prev_message;
/* hook procedures in Hook_Device_Service's format: jmp short past a jmp to the previous handler */
static void __declspec(naked) shell_sysmodal_hook(void)
{
    _asm {
        jmp short body
        jmp dword ptr [shell_prev_sysmodal]
    body:
        pushad
        push eax
        push edi
        push ecx
        push SHELL__SYSMODAL_Message
        cld
        call shell_msg_log
        add esp, 16
        popad
        jmp dword ptr [shell_prev_sysmodal]
    }
}
static void __declspec(naked) shell_message_hook(void)
{
    _asm {
        jmp short body
        jmp dword ptr [shell_prev_message]
    body:
        pushad
        push eax
        push edi
        push ecx
        push SHELL__Message
        cld
        call shell_msg_log
        add esp, 16
        popad
        jmp dword ptr [shell_prev_message]
    }
}
static void shell_hooks(void)
{
    if (!get_ddb(SHELL_DEVICE_ID))
        return;
    shell_prev_sysmodal = hook_service(SHELL_DEVICE_ID << 16 | SHELL__SYSMODAL_Message, shell_sysmodal_hook);
    shell_prev_message = hook_service(SHELL_DEVICE_ID << 16 | SHELL__Message, shell_message_hook);
    log("gen2leg: SHELL message hooks %8 %8\n", shell_prev_sysmodal, shell_prev_message);
}

/* ---- back to DOS ----
 * Windows can exit to real-mode DOS ("Restart in MS-DOS mode", `win` returning), where the BIOS runs on its own
 * again: its tick (synthetic timer 0 in direct mode on vector 0x7d, re-armed by its handler) and the local APIC in
 * xAPIC mode, software-enabled. */
#define BIOS_TICK_PERIOD        549254u         /* one PIT tick in 100 ns, as SeaBIOS */
static u32 bios_stimer_config, bios_apic_base;
static volatile u32 *lapic_mmio;
static void restore_for_dos(void)
{
    exited = 1;
    stimer_stop();
    u32 base = (u32)rdmsr(0x1b);
    wrmsr(0x1b, base & ~0xc00u);
    wrmsr(0x1b, bios_apic_base);
    if (lapic_mmio && (bios_apic_base & 0xc00u) == 0x800u)
        lapic_mmio[0xf0 / 4] = 0x10f;
    else
        wrmsr(0x80f, 0x10f);
    if ((bios_stimer_config & 0x1000u) && (bios_stimer_config >> 4 & 0xff)) {
        wrmsr(HV_MSR_STIMER0_CONFIG, bios_stimer_config | 1);
        wrmsr(HV_MSR_STIMER0_COUNT, ref_now() + BIOS_TICK_PERIOD);
    }
    log("gen2leg: back to DOS: IA32_APIC_BASE %8, BIOS tick %8/%8\n", (u32)rdmsr(0x1b), (u32)rdmsr(HV_MSR_STIMER0_CONFIG),
        (u32)rdmsr(HV_MSR_STIMER0_COUNT));
}

static void init(void)
{
    log("gen2leg: Sys_Critical_Init, APIC base %8, STIMER0 %8/%8\n", (u32)rdmsr(0x1b),
        (u32)rdmsr(HV_MSR_STIMER0_CONFIG), (u32)rdmsr(HV_MSR_STIMER0_COUNT));
    bios_stimer_config = (u32)rdmsr(HV_MSR_STIMER0_CONFIG);
    bios_apic_base = (u32)rdmsr(0x1b);
    stimer_stop();                      /* the BIOS tick: nothing in Windows handles its vector */
    /* A BIOS tick (vector 0x7d) that was pending when VMM took over would be delivered as soon as interrupts
     * are on, through an IDT entry that does not exist (VMM's IDT has 96 gates): #GP, "Windows protection error".
     * VPICD used to switch the local APIC off, which dropped it; VPICD no longer does (we need the APIC for the
     * timer interrupt), so do the same here: APIC off, on again in its reset state, then x2APIC mode (no MMIO
     * mapping needed) and the spurious-interrupt vector register (software enable, vector 0x0f as CSMWrap had). */
    {
        u32 base = (u32)rdmsr(0x1b);
        wrmsr(0x1b, base & ~0xc00u);
        wrmsr(0x1b, (base & ~0xc00u) | 0x800u);
        wrmsr(0x1b, (base & ~0xc00u) | 0xc00u);
        wrmsr(0x80f, 0x10f);
        log("gen2leg: APIC reset, IA32_APIC_BASE %8 -> %8, x2APIC id %8 SVR %8\n", base, (u32)rdmsr(0x1b), (u32)rdmsr(0x802),
            (u32)rdmsr(0x80f));
        lapic_dump("after reset");
    }
    pic[0].imr = pic[1].imr = 0xff;     /* the state after the 8259 init sequence in VPICD's init code */
    pic[0].base = 0x50;
    pic[1].base = 0x58;
    install_gates();
    log("gen2leg: ready, time ref %8\n", (u32)ref_now());
}

void __cdecl control_msg(u32 msg)
{
    hook_nmi();
    static u8 seen_msg[32];
    if (msg == 1) {
        lapic_mmio = (volatile u32 *)vmbc_map(0xfee00000u, 4096);
        vmb_setup();
        shell_hooks();
    }
#ifdef EXIT_TEST
    if (msg == 2)
        exit_at = ref_now() + 400000000u;   /* 40 s */
#endif
    if (msg == 5)                       /* System_Exit */
        mouse_close();
    if (msg == 6)                       /* Sys_Critical_Exit */
        restore_for_dos();
    if (msg == 0)
        init();
    else if (msg < 32 && !seen_msg[msg]++)
    {
        log("gen2leg: control message %x (eoi %x) APIC_BASE %8 STIMER0 %8/%8 now %8 expiry %8 armed %x\n", msg, eoi_count,
            (u32)rdmsr(0x1b), (u32)rdmsr(HV_MSR_STIMER0_CONFIG), (u32)rdmsr(HV_MSR_STIMER0_COUNT), (u32)ref_now(), (u32)t0_expiry, t0_armed);
        lapic_dump("msg");
    }
#ifdef SELFTEST_NMI
    if (msg == 0x1e) {
        log("gen2leg: self test: int 2\n");
        _asm int 2
        log("gen2leg: self test returned\n");
    }
#endif
}


/* ---- entry stubs (naked) ---- */

/* Frame at io_common: op, eip, cs, eflags (the int was taken at ring 0: no stack switch). The caller's DS
 * need not be flat: SYSDETMG.DLL's port thunk is 16-bit ring-0 code running with its own data segment, so
 * load DS/ES from SS (the flat ring-0 stack segment) for the C code and put them back afterwards. */
static void __declspec(naked) io_common(void)
{
    _asm {
        pushad
        push ds
        push es
        push ss
        pop ds
        push ss
        pop es
        lea eax, [esp + 8]              ; the pushad frame (struct regs)
        push eax
        cld
        call shim_int
        add esp, 4
        pop es
        pop ds
        popad
        add esp, 4                      ; op
        iretd
    }
}

#define STUB(n) static void __declspec(naked) stub##n(void) { _asm push n _asm jmp io_common }
STUB(0) STUB(1) STUB(2) STUB(3) STUB(4) STUB(5) STUB(6) STUB(7)
STUB(8) STUB(9) STUB(10) STUB(11) STUB(12) STUB(13) STUB(14) STUB(15)
STUB(16) STUB(17) STUB(18) STUB(19)
STUB(20) STUB(21)
void (*const stubs[NVEC])(void) = {
    stub0, stub1, stub2, stub3, stub4, stub5, stub6, stub7,
    stub8, stub9, stub10, stub11, stub12, stub13, stub14, stub15,
    stub16, stub17, stub18, stub19, stub20, stub21,
};

/* shim_int() makes patched `in/out dx ; ret` sites return through here: the frame's eip is pointed at this
 * thunk and esp is still the interrupted one, so the original return address is on top. */
void __declspec(naked) ret_thunk(void)
{
    _asm ret
}

/* Wrapper in front of the IRQ 0 gate: count the interrupt, then continue at VPICD's handler. */
void __declspec(naked) irq0_wrap(void)
{
    /* Entered from V86 mode or ring 3 the CPU has zeroed DS/ES/FS/GS: use SS (always the flat data segment at
     * ring 0) for the C code, restore them, and reach VPICD's handler through CS. */
    _asm {
        pushad
        push ds
        push es
        push ss
        pop ds
        push ss
        pop es
        cld
        lea eax, [esp + 8]              ; the pushad frame, as irq0_hook expects
        push eax
        call irq0_hook
        add esp, 4
        pop es
        pop ds
        popad
        jmp dword ptr cs:[irq0_orig]
    }
}

/* NMI (injected from the host with Debug-VM): report where the CPU is, then continue at VMM's handler. */
void __declspec(naked) nmi_wrap(void)
{
    _asm {
        pushad
        push ds
        push es
        push ss
        pop ds
        push ss
        pop es
        cld
        lea eax, [esp + 40]             ; the NMI frame: eip, cs, eflags
        push eax
        call nmi_hook
        add esp, 4
        pop es
        pop ds
        popad
        jmp dword ptr cs:[nmi_orig]
    }
}

/* DDB_Control_Proc: eax = message. Preserves registers, returns with carry clear. */
void __declspec(naked) VXD_control(void)
{
    _asm pushad
    _asm push eax
    _asm cld
    _asm call control_msg
    _asm add esp, 4
    _asm popad
    _asm clc
    _asm ret
}

/* Global event callback for mouse reports (VMM: ebx = VM, edx = reference data, ebp = client registers). */
static void __declspec(naked) mouse_event_thunk(void)
{
    _asm pushad
    _asm cld
    _asm call mouse_event_cb
    _asm popad
    _asm ret
}

#ifdef EXIT_TEST
static void __declspec(naked) __cdecl fatal_exit(const char *msg)
{
    _asm {
        mov esi, [esp + 4]
        xor eax, eax
    }
    VMMCall(Fatal_Error_Handler)
    _asm ret
}
void __cdecl exit_event_cb(void)
{
    log("gen2leg: exit test: Fatal_Error_Handler\n");
    fatal_exit("gen2leg exit test: back to DOS");
}
static void __declspec(naked) exit_event_thunk(void)
{
    _asm pushad
    _asm cld
    _asm call exit_event_cb
    _asm popad
    _asm ret
}
#endif
