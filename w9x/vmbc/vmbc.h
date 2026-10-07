// vmbc - VMBus client for real-mode operating systems (DOS, Windows 3.x/9x
// VxDs) that keep using the connection the BIOS made instead of making their
// own: the host sends the channel offers only once per connection, and a new
// connection would cut off the BIOS's own channels (INT 13h storvsc, INT 16h
// keyboard), which DOS still needs after Windows exits.
//
// SeaBIOS publishes the connection in a handoff block (signature "$HvVmBus"
// on a 16-byte boundary of the f-segment). The client takes over the message
// SINT only while it has messages to exchange, opens its own channels with
// GPADL handles the BIOS did not use, may read the rings of channels the BIOS
// opened (handoff offer "chan"), and puts everything back before the BIOS
// runs on its own again.
//
// Polling only: nothing here waits; the caller feeds polled messages to
// vmbc_chan_msg() and checks the channel state. No 64-bit arithmetic (VxDs
// built with Open Watcom have no runtime for it).
//
// The platform supplies the hooks at the end of this file.

#ifndef VMBC_H
#define VMBC_H

#ifndef VMBC_HAVE_TYPES
typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
#endif

#define VMBC_PAGE                       4096

// Interface types
#define VMBC_GUID_KEYBOARD  "\x6d\xad\x12\xf9\x17\x2b\xea\x48\xbd\x65\xf9\x27\xa6\x1c\x76\x84"
#define VMBC_GUID_MOUSE     "\x9e\xb6\xa8\xcf\x4a\x5b\xc0\x4c\xb9\x8b\x8b\xa1\xa1\xf3\xf9\x5a"

#pragma pack(push, 1)

// SeaBIOS src/hw/vmbus.c, revision 1
struct vmbus_handoff_offer {
    u8 if_type[16];
    u8 if_instance[16];
    u32 relid;
    u32 connid;
    u32 chan;                   // struct vmbus_bios_channel, 0 = not opened
};

struct vmbus_handoff {
    u8 signature[8];
    u8 length;
    u8 revision;
    u8 sint;
    u8 offer_count;
    u32 version;
    u32 msg_conn_id;
    u32 vp_index;
    u32 int_page;
    u32 monitor_pages;
    u32 offers;
    u32 next_gpadl;
};

// SeaBIOS struct vmbus_channel (pointers are physical addresses)
struct vmbus_bios_channel {
    u32 child_relid;
    u32 connection_id;
    u32 out, in;
    u32 out_size, in_size;
};

struct hv_message {
    u32 type;
    u8 payload_size;
    u8 flags;
    u16 reserved;
    u32 origin[2];
    u8 payload[240];
};

struct vmbus_ring {
    u32 write_index;
    u32 read_index;
    u32 interrupt_mask;
    u32 pending_send_sz;
    u32 reserved1[12];
    u32 feature_bits;
    u8 reserved2[VMBC_PAGE - 68];
    u8 buffer[1];
};

struct vmpacket_descriptor {
    u16 type;
    u16 offset8;
    u16 len8;
    u16 flags;
    u32 trans_id[2];
};

#pragma pack(pop)

// Channel message types
#define CHANNELMSG_OFFERCHANNEL         1
#define CHANNELMSG_RESCIND_CHANNELOFFER 2
#define CHANNELMSG_OPENCHANNEL          5
#define CHANNELMSG_OPENCHANNEL_RESULT   6
#define CHANNELMSG_CLOSECHANNEL         7
#define CHANNELMSG_GPADL_HEADER         8
#define CHANNELMSG_GPADL_CREATED        10
#define CHANNELMSG_GPADL_TEARDOWN       11
#define CHANNELMSG_GPADL_TORNDOWN       12

// Packet types
#define VM_PKT_DATA_INBAND              6
#define VM_PKT_COMP                     11
#define VMBUS_DATA_PACKET_FLAG_COMPLETION_REQUESTED 1

struct vmbc {
    struct vmbus_handoff *ho;
    struct vmbus_handoff_offer *offers;
    // message pages while vmbc_msg_begin() is in effect
    struct hv_message *slot;
    u8 *post;
    u32 post_phys;
    u32 saved[3][2];            // SIMP, SIEFP, SINT
    u8 *send_int;               // guest-to-host half of the interrupt page
    u32 next_gpadl;
    u32 scratch[60];            // messages being built (VxD stacks are small)
};

enum {
    VMBC_CLOSED, VMBC_GPADL, VMBC_OPENING, VMBC_OPEN, VMBC_TEARDOWN, VMBC_FAILED
};

struct vmbc_chan {
    int state, has_gpadl;
    u32 relid, connid, gpadl, status;
    struct vmbus_ring *out, *in;
    u32 out_size, in_size;      // data area sizes in bytes
};

// Find the BIOS's handoff block. Returns 0 or -1.
int vmbc_init(struct vmbc *c);
// Offer number n of the given interface type, or 0.
struct vmbus_handoff_offer *vmbc_offer(struct vmbc *c, const char *guid, int n);
// Adopt the rings of a channel the BIOS opened (state VMBC_OPEN).
int vmbc_adopt(struct vmbc *c, struct vmbc_chan *ch, struct vmbus_handoff_offer *o);

// Receive channel messages on three zeroed pages (SIMP, SIEFP, post input),
// SINT unmasked on vector (AutoEOI; it only has to exist, messages are
// polled), until vmbc_msg_end() puts the BIOS's MSRs back.
void vmbc_msg_begin(struct vmbc *c, void *pages, u32 vector);
void vmbc_msg_end(struct vmbc *c);
// HvPostMessage; returns the hypervisor status (0 = ok).
int vmbc_post(struct vmbc *c, const void *msg, u32 len);
// Copy the next channel message (240 bytes) to buf; returns its type or -1.
int vmbc_poll_msg(struct vmbc *c, void *buf);

// Open a channel with both rings (ring_pages each, header page included) in
// zeroed pages; the state goes GPADL -> OPENING -> OPEN or FAILED as the
// replies are fed to vmbc_chan_msg().
int vmbc_open(struct vmbc *c, struct vmbc_chan *ch, struct vmbus_handoff_offer *o
              , void *ring, u32 ring_pages);
// Close a channel opened by vmbc_open(); TEARDOWN -> CLOSED.
int vmbc_close(struct vmbc *c, struct vmbc_chan *ch);
// Feed a polled channel message; returns 1 if it was for this channel.
int vmbc_chan_msg(struct vmbc *c, struct vmbc_chan *ch, const void *msg, int type);

// Queue an in-band packet; returns 0 or -1 (ring full).
int vmbc_send(struct vmbc *c, struct vmbc_chan *ch, u16 type, u16 flags
              , const void *data, u32 len, u32 trans_id);
// Dequeue one packet: payload copied up to maxlen; returns its length or -1.
int vmbc_recv(struct vmbc_chan *ch, void *data, u32 maxlen, u16 *type);

// Platform hooks
u32 vmbc_rdmsr(u32 msr, u32 *hi);
void vmbc_wrmsr(u32 msr, u32 lo, u32 hi);
// Hypercall through the hypercall page: control in eax, input in ecx, output
// in esi (edx, ebx, edi zero); returns eax.
u32 vmbc_hypercall(u32 control, u32 input, u32 output);
void *vmbc_map(u32 phys, u32 len);
u32 vmbc_phys(void *p);
// Full memory barrier (a locked instruction)
void vmbc_mb(void);

#endif
