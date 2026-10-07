// vmbc - VMBus client on the BIOS's connection (see vmbc.h)

#include "vmbc.h"

#define HV_MSR_SCONTROL                 0x40000080
#define HV_MSR_SIEFP                    0x40000082
#define HV_MSR_SIMP                     0x40000083
#define HV_MSR_EOM                      0x40000084
#define HV_MSR_SINT0                    0x40000090
#define HV_SINT_AUTO_EOI                (1 << 17)

#define HVCALL_POST_MESSAGE             0x005c
#define HVCALL_SIGNAL_EVENT             0x005d
#define HV_HYPERCALL_FAST               (1 << 16)

#define HV_MESSAGE_FLAG_PENDING         1

#define GPADL_MAX_PFNS                  26

#pragma pack(push, 1)
struct msg_header { u32 msgtype, padding; };
struct msg_gpadl_header {
    struct msg_header h;
    u32 child_relid, gpadl;
    u16 range_buflen, rangecount;
    u32 byte_count, byte_offset;
    u32 pfn[GPADL_MAX_PFNS][2];
};
struct msg_gpadl_created { struct msg_header h; u32 child_relid, gpadl, status; };
struct msg_open_channel {
    struct msg_header h;
    u32 child_relid, openid, ringbuffer_gpadl, target_vp, downstream_ring_pageoffset;
    u8 user_data[120];
};
struct msg_open_result { struct msg_header h; u32 child_relid, openid, status; };
struct msg_close_channel { struct msg_header h; u32 child_relid; };
struct msg_gpadl_teardown { struct msg_header h; u32 child_relid, gpadl; };
struct msg_gpadl_torndown { struct msg_header h; u32 gpadl; };
#pragma pack(pop)

static void
copy(void *dst, const void *src, u32 n)
{
    u8 *d = dst;
    const u8 *s = src;
    while (n--)
        *d++ = *s++;
}

static void
zero(void *dst, u32 n)
{
    u8 *d = dst;
    while (n--)
        *d++ = 0;
}

static int
same(const void *a, const void *b, u32 n)
{
    const u8 *x = a, *y = b;
    while (n--)
        if (*x++ != *y++)
            return 0;
    return 1;
}

int
vmbc_init(struct vmbc *c)
{
    u8 *f = vmbc_map(0xf0000, 0x10000);
    u32 i;
    zero(c, sizeof(*c));
    for (i = 0; i < 0x10000; i += 16) {
        struct vmbus_handoff *ho = (void *)(f + i);
        if (!same(ho->signature, "$HvVmBus", 8) || ho->revision < 1
            || ho->length < sizeof(*ho))
            continue;
        c->ho = ho;
        c->offers = vmbc_map(ho->offers
                             , ho->offer_count * sizeof(struct vmbus_handoff_offer));
        c->send_int = vmbc_map(ho->int_page + VMBC_PAGE / 2, VMBC_PAGE / 2);
        c->next_gpadl = ho->next_gpadl;
        return 0;
    }
    return -1;
}

struct vmbus_handoff_offer *
vmbc_offer(struct vmbc *c, const char *guid, int n)
{
    u32 i;
    for (i = 0; i < c->ho->offer_count; i++)
        if (same(c->offers[i].if_type, guid, 16) && n-- == 0)
            return &c->offers[i];
    return 0;
}

int
vmbc_adopt(struct vmbc *c, struct vmbc_chan *ch, struct vmbus_handoff_offer *o)
{
    struct vmbus_bios_channel *bc;
    zero(ch, sizeof(*ch));
    if (!o->chan)
        return -1;
    bc = vmbc_map(o->chan, sizeof(*bc));
    ch->relid = bc->child_relid;
    ch->connid = bc->connection_id;
    ch->out_size = bc->out_size;
    ch->in_size = bc->in_size;
    ch->out = vmbc_map(bc->out, VMBC_PAGE + bc->out_size);
    ch->in = vmbc_map(bc->in, VMBC_PAGE + bc->in_size);
    ch->state = VMBC_OPEN;
    return 0;
}


/****************************************************************
 * Channel messages
 ****************************************************************/

void
vmbc_msg_begin(struct vmbc *c, void *pages, u32 vector)
{
    u8 *p = pages;
    u32 sint = HV_MSR_SINT0 + c->ho->sint, hi;
    c->saved[0][0] = vmbc_rdmsr(HV_MSR_SIMP, &c->saved[0][1]);
    c->saved[1][0] = vmbc_rdmsr(HV_MSR_SIEFP, &c->saved[1][1]);
    c->saved[2][0] = vmbc_rdmsr(sint, &c->saved[2][1]);
    c->slot = (struct hv_message *)p + c->ho->sint;
    c->post = p + 2 * VMBC_PAGE;
    c->post_phys = vmbc_phys(c->post);
    vmbc_wrmsr(HV_MSR_SIMP, vmbc_phys(p) | 1, 0);
    vmbc_wrmsr(HV_MSR_SIEFP, vmbc_phys(p + VMBC_PAGE) | 1, 0);
    vmbc_wrmsr(sint, vector | HV_SINT_AUTO_EOI, 0);
    vmbc_wrmsr(HV_MSR_SCONTROL, vmbc_rdmsr(HV_MSR_SCONTROL, &hi) | 1, hi);
}

void
vmbc_msg_end(struct vmbc *c)
{
    vmbc_wrmsr(HV_MSR_SINT0 + c->ho->sint, c->saved[2][0], c->saved[2][1]);
    vmbc_wrmsr(HV_MSR_SIEFP, c->saved[1][0], c->saved[1][1]);
    vmbc_wrmsr(HV_MSR_SIMP, c->saved[0][0], c->saved[0][1]);
    c->slot = 0;
}

int
vmbc_post(struct vmbc *c, const void *msg, u32 len)
{
    u32 *in = (u32 *)c->post;
    in[0] = c->ho->msg_conn_id;
    in[1] = 0;
    in[2] = 1;                  // message type: channel message
    in[3] = len;
    copy(in + 4, msg, len);
    return vmbc_hypercall(HVCALL_POST_MESSAGE, c->post_phys, 0) & 0xffff;
}

int
vmbc_poll_msg(struct vmbc *c, void *buf)
{
    struct hv_message *m = c->slot;
    if (!*(volatile u32 *)&m->type)
        return -1;
    copy(buf, m->payload, sizeof(m->payload));
    // Free the slot before looking at MessagePending, so a message queued
    // meanwhile gets delivered after the EOM.
    *(volatile u32 *)&m->type = 0;
    vmbc_mb();
    if (*(volatile u8 *)&m->flags & HV_MESSAGE_FLAG_PENDING)
        vmbc_wrmsr(HV_MSR_EOM, 0, 0);
    return ((struct msg_header *)buf)->msgtype;
}


/****************************************************************
 * Opening and closing channels
 ****************************************************************/

int
vmbc_open(struct vmbc *c, struct vmbc_chan *ch, struct vmbus_handoff_offer *o
          , void *ring, u32 ring_pages)
{
    struct msg_gpadl_header *m = (void *)c->scratch;
    u32 pages = 2 * ring_pages, i;
    zero(ch, sizeof(*ch));
    if (pages > GPADL_MAX_PFNS)
        return -1;
    ch->relid = o->relid;
    ch->connid = o->connid;
    ch->gpadl = c->next_gpadl++;
    ch->out = ring;
    ch->in = (struct vmbus_ring *)((u8 *)ring + ring_pages * VMBC_PAGE);
    ch->out_size = ch->in_size = (ring_pages - 1) * VMBC_PAGE;
    // Polled: no interrupts for packets from the host
    ch->in->interrupt_mask = 1;

    zero(m, sizeof(*m));
    m->h.msgtype = CHANNELMSG_GPADL_HEADER;
    m->child_relid = ch->relid;
    m->gpadl = ch->gpadl;
    m->range_buflen = 8 + pages * 8;
    m->rangecount = 1;
    m->byte_count = pages * VMBC_PAGE;
    for (i = 0; i < pages; i++)
        m->pfn[i][0] = vmbc_phys((u8 *)ring + i * VMBC_PAGE) >> 12;
    ch->state = VMBC_GPADL;
    if ((ch->status = vmbc_post(c, m, 28 + pages * 8)) != 0) {
        ch->state = VMBC_FAILED;
        return -1;
    }
    return 0;
}

static int
post_teardown(struct vmbc *c, struct vmbc_chan *ch)
{
    struct msg_gpadl_teardown *t = (void *)c->scratch;
    zero(t, sizeof(*t));
    t->h.msgtype = CHANNELMSG_GPADL_TEARDOWN;
    t->child_relid = ch->relid;
    t->gpadl = ch->gpadl;
    ch->state = VMBC_TEARDOWN;
    if (vmbc_post(c, t, sizeof(*t))) {
        ch->state = VMBC_FAILED;
        return -1;
    }
    return 0;
}

int
vmbc_close(struct vmbc *c, struct vmbc_chan *ch)
{
    if (ch->state == VMBC_OPEN) {
        struct msg_close_channel *m = (void *)c->scratch;
        zero(m, sizeof(*m));
        m->h.msgtype = CHANNELMSG_CLOSECHANNEL;
        m->child_relid = ch->relid;
        if (vmbc_post(c, m, sizeof(*m)))
            return -1;
    } else if (!ch->has_gpadl) {
        ch->state = VMBC_CLOSED;
        return 0;
    }
    return post_teardown(c, ch);
}

int
vmbc_chan_msg(struct vmbc *c, struct vmbc_chan *ch, const void *msg, int type)
{
    const struct msg_gpadl_created *gc = msg;
    const struct msg_open_result *or = msg;
    const struct msg_gpadl_torndown *td = msg;
    switch (type) {
    case CHANNELMSG_GPADL_CREATED:
        if (ch->state != VMBC_GPADL || gc->child_relid != ch->relid
            || gc->gpadl != ch->gpadl)
            return 0;
        if ((ch->status = gc->status) != 0) {
            ch->state = VMBC_FAILED;
        } else {
            struct msg_open_channel *m = (void *)c->scratch;
            ch->has_gpadl = 1;
            zero(m, sizeof(*m));
            m->h.msgtype = CHANNELMSG_OPENCHANNEL;
            m->child_relid = ch->relid;
            m->openid = ch->relid;
            m->ringbuffer_gpadl = ch->gpadl;
            m->target_vp = c->ho->vp_index;
            m->downstream_ring_pageoffset = ch->out_size / VMBC_PAGE + 1;
            ch->state = VMBC_OPENING;
            if ((ch->status = vmbc_post(c, m, sizeof(*m))) != 0)
                ch->state = VMBC_FAILED;
        }
        return 1;
    case CHANNELMSG_OPENCHANNEL_RESULT:
        if (ch->state != VMBC_OPENING || or->child_relid != ch->relid)
            return 0;
        ch->status = or->status;
        ch->state = or->status ? VMBC_FAILED : VMBC_OPEN;
        return 1;
    case CHANNELMSG_GPADL_TORNDOWN:
        if (ch->state != VMBC_TEARDOWN || td->gpadl != ch->gpadl)
            return 0;
        ch->has_gpadl = 0;
        ch->state = VMBC_CLOSED;
        return 1;
    }
    return 0;
}


/****************************************************************
 * Rings
 ****************************************************************/

static u32
ring_put(struct vmbus_ring *r, u32 size, u32 pos, const void *data, u32 len)
{
    const u8 *p = data;
    while (len--) {
        r->buffer[pos] = p ? *p++ : 0;
        if (++pos == size)
            pos = 0;
    }
    return pos;
}

static u32
ring_get(struct vmbus_ring *r, u32 size, u32 pos, void *data, u32 len)
{
    u8 *p = data;
    while (len--) {
        if (p)
            *p++ = r->buffer[pos];
        if (++pos == size)
            pos = 0;
    }
    return pos;
}

int
vmbc_send(struct vmbc *c, struct vmbc_chan *ch, u16 type, u16 flags
          , const void *data, u32 len, u32 trans_id)
{
    struct vmbus_ring *r = ch->out;
    u32 size = ch->out_size;
    u32 pktlen = (sizeof(struct vmpacket_descriptor) + len + 7) & ~7;
    u32 write = r->write_index, read = *(volatile u32 *)&r->read_index;
    u32 avail = write >= read ? size - (write - read) : read - write;
    struct vmpacket_descriptor desc;
    u32 prev[2], pos;
    if (pktlen + 8 >= avail)
        return -1;
    desc.type = type;
    desc.offset8 = sizeof(desc) / 8;
    desc.len8 = pktlen / 8;
    desc.flags = flags;
    desc.trans_id[0] = trans_id;
    desc.trans_id[1] = 0;
    pos = ring_put(r, size, write, &desc, sizeof(desc));
    pos = ring_put(r, size, pos, data, len);
    pos = ring_put(r, size, pos, 0, pktlen - sizeof(desc) - len);
    prev[0] = 0;
    prev[1] = write;
    pos = ring_put(r, size, pos, prev, sizeof(prev));
    vmbc_mb();
    *(volatile u32 *)&r->write_index = pos;
    vmbc_mb();

    // The host only expects a signal when the ring goes from empty to
    // non-empty, and not at all while it has masked interrupts.
    if (*(volatile u32 *)&r->interrupt_mask
        || *(volatile u32 *)&r->read_index != write)
        return 0;
    c->send_int[ch->relid / 8] |= 1 << (ch->relid % 8);
    vmbc_hypercall(HVCALL_SIGNAL_EVENT | HV_HYPERCALL_FAST, ch->connid, 0);
    return 0;
}

int
vmbc_recv(struct vmbc_chan *ch, void *data, u32 maxlen, u16 *type)
{
    struct vmbus_ring *r = ch->in;
    u32 size = ch->in_size;
    u32 read = r->read_index;
    struct vmpacket_descriptor desc;
    u32 offset, len, pos;
    if (read == *(volatile u32 *)&r->write_index)
        return -1;
    vmbc_mb();
    ring_get(r, size, read, &desc, sizeof(desc));
    offset = desc.offset8 * 8;
    len = desc.len8 * 8 - offset;
    pos = ring_get(r, size, read, 0, offset);
    pos = ring_get(r, size, pos, data, len < maxlen ? len : maxlen);
    if (len > maxlen)
        pos = ring_get(r, size, pos, 0, len - maxlen);
    pos = ring_get(r, size, pos, 0, 8);
    vmbc_mb();
    *(volatile u32 *)&r->read_index = pos;
    *type = desc.type;
    return len;
}
