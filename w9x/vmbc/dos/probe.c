// vmbc DOS probe: on the connection SeaBIOS made, read the handoff block,
// look at the keyboard channel the BIOS opened, then open the synthetic
// mouse channel twice (handshake, input reports, close) and put the SynIC
// MSRs back. Runs from stub.asm in flat 32-bit protected mode; output on
// COM1. Move the mouse (e.g. Msvm_SyntheticMouse.SetAbsolutePosition)
// while "MOUSE-READY" is up.

#include "../vmbc.h"

#define HV_MSR_HYPERCALL        0x40000001
#define HV_MSR_TIME_REF_COUNT   0x40000020
#define VMBUS_SINT_VECTOR       0xf9    // SeaBIOS points it at an iret

static u32 base;                // linear address of our segment
static u32 hc_off;              // hypercall page as an offset in it

void *memcpy(void *d, const void *s, unsigned n)
{
    u8 *dd = d;
    const u8 *ss = s;
    while (n--)
        *dd++ = *ss++;
    return d;
}

void *memset(void *d, int v, unsigned n)
{
    u8 *dd = d;
    while (n--)
        *dd++ = v;
    return d;
}

/****************************************************************
 * Platform hooks
 ****************************************************************/

u32 vmbc_rdmsr(u32 msr, u32 *hi)
{
    u32 lo, h;
    asm volatile("rdmsr" : "=a"(lo), "=d"(h) : "c"(msr));
    if (hi)
        *hi = h;
    return lo;
}

void vmbc_wrmsr(u32 msr, u32 lo, u32 hi)
{
    asm volatile("wrmsr" :: "c"(msr), "a"(lo), "d"(hi) : "memory");
}

u32 vmbc_hypercall(u32 control, u32 input, u32 output)
{
    u32 a = control, d = 0, c = input, S = output, D = 0;
    asm volatile("call *%[pg]"
                 : "+a"(a), "+d"(d), "+c"(c), "+S"(S), "+D"(D)
                 : "b"(0), [pg] "m"(hc_off)
                 : "memory", "cc");
    return a;
}

void *vmbc_map(u32 phys, u32 len)
{
    (void)len;
    return (void *)(phys - base);
}

u32 vmbc_phys(void *p)
{
    return (u32)p + base;
}

void vmbc_mb(void)
{
    asm volatile("lock; addl $0, (%%esp)" ::: "memory", "cc");
}

/****************************************************************
 * Output
 ****************************************************************/

static void outb(u16 port, u8 v) { asm volatile("outb %0, %1" :: "a"(v), "Nd"(port)); }
static u8 inb(u16 port) { u8 v; asm volatile("inb %1, %0" : "=a"(v) : "Nd"(port)); return v; }

static void putch(char ch)
{
    if (ch == '\n')
        putch('\r');
    while (!(inb(0x3fd) & 0x20))
        ;
    outb(0x3f8, ch);
}

// %x %u %d %s %c with optional zero padding and width
static void pr(const char *fmt, ...)
{
    __builtin_va_list ap;
    __builtin_va_start(ap, fmt);
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            putch(*fmt);
            continue;
        }
        char pad = ' ', buf[12];
        int width = 0, n = 0;
        if (*++fmt == '0')
            pad = '0';
        while (*fmt >= '0' && *fmt <= '9')
            width = width * 10 + *fmt++ - '0';
        u32 v;
        switch (*fmt) {
        case 's': {
            const char *s = __builtin_va_arg(ap, const char *);
            while (*s)
                putch(*s++);
            continue;
        }
        case 'c':
            putch(__builtin_va_arg(ap, int));
            continue;
        case 'd':
            v = __builtin_va_arg(ap, int);
            if ((int)v < 0) {
                putch('-');
                v = -v;
            }
            do buf[n++] = '0' + v % 10; while (v /= 10);
            break;
        case 'u':
            v = __builtin_va_arg(ap, u32);
            do buf[n++] = '0' + v % 10; while (v /= 10);
            break;
        default:
            v = __builtin_va_arg(ap, u32);
            do buf[n++] = "0123456789abcdef"[v & 15]; while (v >>= 4);
            break;
        }
        while (width-- > n)
            putch(pad);
        while (n)
            putch(buf[--n]);
    }
    __builtin_va_end(ap);
}

static void hexdump(const char *tag, const u8 *p, u32 n)
{
    u32 i;
    for (i = 0; i < n; i++) {
        if (i % 32 == 0)
            pr("%s%s", i ? "\n" : "", tag);
        pr(" %02x", p[i]);
    }
    pr("\n");
}

static void guid(const u8 *g)
{
    pr("%08x-%04x-%04x-%02x%02x-", *(u32 *)g, *(u16 *)(g + 4), *(u16 *)(g + 6)
       , g[8], g[9]);
    int i;
    for (i = 10; i < 16; i++)
        pr("%02x", g[i]);
}

static u32 now(void)            // 100 ns units, wraps after 7 minutes
{
    return vmbc_rdmsr(HV_MSR_TIME_REF_COUNT, 0);
}

/****************************************************************
 * Synthetic HID (mouse)
 ****************************************************************/

#define PIPE_MESSAGE_DATA               1
#define HID_PROTOCOL_REQUEST            0
#define HID_PROTOCOL_RESPONSE           1
#define HID_INITIAL_DEVICE_INFO         2
#define HID_INITIAL_DEVICE_INFO_ACK     3
#define HID_INPUT_REPORT                4
#define HID_VERSION                     0x00020000

struct hid_msg {
    u32 pipe_type, pipe_size;   // pipe_prt_msg
    u32 type, size;             // synthhid_msg_hdr
    u8 data[1000];
} __attribute__((packed));

#define MOUSE_RING_PAGES        4

static int wait_state(struct vmbc *c, struct vmbc_chan *ch, int want, u32 ms)
{
    u8 buf[240];
    u32 t0 = now();
    while (ch->state != want && ch->state != VMBC_FAILED) {
        int t = vmbc_poll_msg(c, buf);
        if (t >= 0) {
            if (vmbc_chan_msg(c, ch, buf, t))
                pr("  msg %d -> state %d status %x\n", t, ch->state, ch->status);
            else
                pr("  msg %d ignored\n", t);
        }
        if (now() - t0 > ms * 10000) {
            pr("  timeout in state %d\n", ch->state);
            return -1;
        }
    }
    return ch->state == want ? 0 : -1;
}

// Protocol request, response, device info, ack. Returns 0 when acked.
static int hid_handshake(struct vmbc *c, struct vmbc_chan *ch)
{
    struct hid_msg m;
    memset(&m, 0, sizeof(m));
    m.pipe_type = PIPE_MESSAGE_DATA;
    m.pipe_size = 12;
    m.type = HID_PROTOCOL_REQUEST;
    m.size = 4;
    *(u32 *)m.data = HID_VERSION;
    if (vmbc_send(c, ch, VM_PKT_DATA_INBAND
                  , VMBUS_DATA_PACKET_FLAG_COMPLETION_REQUESTED, &m, 20, 1)) {
        pr("  send failed\n");
        return -1;
    }
    u32 t0 = now();
    int approved = 0;
    while (now() - t0 < 50000000) {
        u16 type;
        int len = vmbc_recv(ch, &m, sizeof(m), &type);
        if (len < 0)
            continue;
        if (type != VM_PKT_DATA_INBAND || len < 16) {
            pr("  packet type %d len %d\n", type, len);
            continue;
        }
        pr("  hid msg %d size %d\n", m.type, m.size);
        if (m.type == HID_PROTOCOL_RESPONSE) {
            approved = m.data[4];
            pr("  protocol %x approved %d\n", *(u32 *)m.data, approved);
            if (!approved)
                return -1;
        } else if (m.type == HID_INITIAL_DEVICE_INFO) {
            // hv_input_dev_info (u32 size, u16 vendor, product, version,
            // reserved[11]), then the HID descriptor, then the report one
            u16 *info = (u16 *)m.data;
            u8 *desc = m.data + 32;
            u32 rlen = desc[7] | desc[8] << 8;
            pr("  device %04x:%04x version %x, hid desc len %d, report desc %d bytes\n"
               , info[2], info[3], info[4], desc[0], rlen);
            hexdump("  rdesc", desc + desc[0], rlen > 160 ? 160 : rlen);
            memset(&m, 0, sizeof(m));
            m.pipe_type = PIPE_MESSAGE_DATA;
            m.pipe_size = 9;
            m.type = HID_INITIAL_DEVICE_INFO_ACK;
            m.size = 1;
            if (vmbc_send(c, ch, VM_PKT_DATA_INBAND
                          , VMBUS_DATA_PACKET_FLAG_COMPLETION_REQUESTED, &m, 17, 2))
                return -1;
            return approved ? 0 : -1;
        }
    }
    pr("  handshake timeout\n");
    return -1;
}

static void hid_reports(struct vmbc_chan *ch, u32 seconds)
{
    struct hid_msg m;
    u32 t0 = now(), count = 0;
    pr("vmbc: MOUSE-READY\n");
    while (now() - t0 < seconds * 10000000) {
        u16 type;
        int len = vmbc_recv(ch, &m, sizeof(m), &type);
        if (len < 0)
            continue;
        if (type != VM_PKT_DATA_INBAND || m.type != HID_INPUT_REPORT)
            continue;
        if (count++ < 40)
            hexdump("  report", m.data, m.size);
    }
    pr("vmbc: MOUSE-DONE, %u reports\n", count);
}

/****************************************************************
 * Main
 ****************************************************************/

static void ring_state(const char *tag, struct vmbc_chan *ch)
{
    pr("%s: in r/w %x/%x mask %d, out r/w %x/%x\n", tag, ch->in->read_index
       , ch->in->write_index, ch->in->interrupt_mask, ch->out->read_index
       , ch->out->write_index);
}

__attribute__((section(".text.entry"))) int
probe_main(u32 seg_base)
{
    base = seg_base;
    pr("vmbc: probe, segment base %x\n", base);
    u32 hi, hc = vmbc_rdmsr(HV_MSR_HYPERCALL, &hi);
    if (!(hc & 1)) {
        pr("vmbc: hypercall page not enabled\n");
        return 1;
    }
    hc_off = (hc & ~0xfff) - base;

    struct vmbc c;
    if (vmbc_init(&c)) {
        pr("vmbc: no handoff block\n");
        return 2;
    }
    struct vmbus_handoff *ho = c.ho;
    pr("vmbc: handoff at %x rev %d: version %x conn %d vp %d sint %d int %x monitor %x"
       " next gpadl %x, %d offers\n", vmbc_phys(ho), ho->revision, ho->version
       , ho->msg_conn_id, ho->vp_index, ho->sint, ho->int_page, ho->monitor_pages
       , ho->next_gpadl, ho->offer_count);
    int i;
    for (i = 0; i < ho->offer_count; i++) {
        struct vmbus_handoff_offer *o = &c.offers[i];
        pr("  relid %2d conn %4x chan %8x type ", o->relid, o->connid, o->chan);
        guid(o->if_type);
        pr(" inst ");
        guid(o->if_instance);
        pr("\n");
    }

    struct vmbc_chan kbd;
    struct vmbus_handoff_offer *ko = vmbc_offer(&c, VMBC_GUID_KEYBOARD, 0);
    if (ko && !vmbc_adopt(&c, &kbd, ko))
        ring_state("vmbc: BIOS keyboard ring", &kbd);
    else
        pr("vmbc: no BIOS keyboard channel\n");

    struct vmbus_handoff_offer *mo = vmbc_offer(&c, VMBC_GUID_MOUSE, 0);
    if (!mo) {
        pr("vmbc: no mouse offer\n");
        return 3;
    }
    // Message pages (3) and mouse rings above our 64 KiB segment
    u8 *heap = vmbc_map((base + 0x10000 + 0xfff) & ~0xfff, 0);
    u8 *ring = heap + 3 * VMBC_PAGE;
    int round, fails = 0;
    for (round = 1; round <= 2; round++) {
        struct vmbc_chan ch;
        memset(heap, 0, (3 + 2 * MOUSE_RING_PAGES) * VMBC_PAGE);
        pr("vmbc: round %d: open mouse relid %d, gpadl %x\n", round, mo->relid
           , c.next_gpadl);
        vmbc_msg_begin(&c, heap, VMBUS_SINT_VECTOR);
        if (vmbc_open(&c, &ch, mo, ring, MOUSE_RING_PAGES)) {
            pr("  open post failed %x\n", ch.status);
            fails++;
        } else if (!wait_state(&c, &ch, VMBC_OPEN, 5000)) {
            ring_state("  mouse ring", &ch);
            if (hid_handshake(&c, &ch))
                fails++;
            else
                hid_reports(&ch, round == 1 ? 20 : 2);
            ring_state("  mouse ring", &ch);
        } else {
            fails++;
        }
        pr("vmbc: round %d: close\n", round);
        if (vmbc_close(&c, &ch) || wait_state(&c, &ch, VMBC_CLOSED, 5000))
            fails++;
        vmbc_msg_end(&c);
    }
    if (ko && !vmbc_adopt(&c, &kbd, ko))
        ring_state("vmbc: BIOS keyboard ring", &kbd);
    pr("vmbc: %s\n", fails ? "FAILED" : "all rounds ok");
    return fails ? 4 : 0;
}
