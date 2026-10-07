/*
 * testplay.c: minimal console waveOut player for scripted tests of the
 * PortCls WaveCyclic virtual sound card on Windows XP (x86).  Plays a
 * generated sine tone or a PCM WAV file and prints one grep-friendly line
 * per event on stdout; the exit code is the MMRESULT (0 on success).
 *
 * Build (i686 PE, subsystem 5.01 so that XP will run it):
 *   /opt/msys2-cross/bin/i686-w64-mingw32-gcc -O2 -Wall -Wextra \
 *       -o /tmp/testplay.exe vmbaud/testplay.c -lwinmm \
 *       -Wl,--subsystem,console:5.01 \
 *       -Wl,--major-os-version,5 -Wl,--minor-os-version,1
 *
 * Usage:
 *   testplay [options]
 *     -s N       seconds, default 3
 *     -r N       sample rate, default 48000
 *     -c N       channels, default 2
 *     -b N       bits per sample (8 or 16), default 16
 *     -f FREQ    sine frequency, default 440
 *     -w FILE    play FILE (PCM 16-bit WAV) instead of a generated sine
 *     -n N       number of WAVEHDR buffers kept queued, default 4 (2..32)
 *     -m MS      milliseconds of audio per buffer, default 20 (5..1000)
 *     -p         poll for finished buffers with Sleep(2) instead of waiting
 *                on an event (CALLBACK_EVENT) the way players do; a Sleep
 *                lasts a whole clock tick (15.6 ms at XP's default 64 Hz),
 *                so with small buffers kmixer runs dry every few hundred ms
 *     -t         timeBeginPeriod(1) while playing
 *     -T         print the clock tick (NtQueryTimerResolution) and exit
 *     -l         list waveOut devices and exit
 *     -q         quiet: print only on failure
 *
 * Output lines (stdout, one per event):
 *   DEV <index> <name> <channels> <formats>   (-l; index -1 is the mapper)
 *   OPEN <rate> <channels> <bits> <bytes>
 *   PLAY chunk=<n> bytes=<b>
 *   DONE played=<bytes> ms=<ms>
 *   TIMER min=<100ns> max=<100ns> cur=<100ns>  (-T)
 *   FAIL <what> mmerr=<code>                  (exit code = code)
 *
 * Distributed under the MS-PL; see LICENSE in this directory.
 */
#include <windows.h>
#include <mmsystem.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

#define DEF_SECONDS     3
#define DEF_RATE        48000
#define DEF_CHANNELS    2
#define DEF_BITS        16
#define DEF_FREQ        440.0

#define DEF_CHUNK_MS    20          /* nominal waveOutWrite size */
#define DEF_NBUFS       4           /* recycled WAVEHDR slots */
#define MAX_NBUFS       32
#define MAX_PCM_BYTES   (64u * 1024u * 1024u)

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

static int quiet;
static HANDLE done_event;   /* -e */
static unsigned chunk_ms = DEF_CHUNK_MS;
static unsigned nbufs = DEF_NBUFS;

/* Print a FAIL line; the return value is the process exit code. */
static int fail(const char *what, UINT mmerr)
{
    if (mmerr == 0)
        mmerr = MMSYSERR_ERROR;
    printf("FAIL %s mmerr=%u\n", what, mmerr);
    fflush(stdout);
    return (int)mmerr;
}

static const char *format_bit_name(DWORD bit)
{
    switch (bit) {
    case WAVE_FORMAT_1M08: return "WAVE_FORMAT_1M08";
    case WAVE_FORMAT_1M16: return "WAVE_FORMAT_1M16";
    case WAVE_FORMAT_1S08: return "WAVE_FORMAT_1S08";
    case WAVE_FORMAT_1S16: return "WAVE_FORMAT_1S16";
    case WAVE_FORMAT_2M08: return "WAVE_FORMAT_2M08";
    case WAVE_FORMAT_2M16: return "WAVE_FORMAT_2M16";
    case WAVE_FORMAT_2S08: return "WAVE_FORMAT_2S08";
    case WAVE_FORMAT_2S16: return "WAVE_FORMAT_2S16";
    case WAVE_FORMAT_4M08: return "WAVE_FORMAT_4M08";
    case WAVE_FORMAT_4M16: return "WAVE_FORMAT_4M16";
    case WAVE_FORMAT_4S08: return "WAVE_FORMAT_4S08";
    case WAVE_FORMAT_4S16: return "WAVE_FORMAT_4S16";
    default:               return NULL;
    }
}

/* DEV lines stay one token: spaces in the device name become underscores. */
static void print_dev(UINT_PTR id, const WAVEOUTCAPSA *caps)
{
    char name[MAXPNAMELEN * 2];
    char formats[256];
    int i, n = 0;

    for (i = 0; caps->szPname[i] && i < (int)sizeof(name) - 1; i++) {
        char c = caps->szPname[i];
        name[i] = (c == ' ' || c == '\t') ? '_' : c;
    }
    name[i] = 0;

    formats[0] = 0;
    for (i = 0; i < 12; i++) {
        DWORD bit = 1u << i;
        const char *nm;
        if (!(caps->dwFormats & bit))
            continue;
        nm = format_bit_name(bit);
        if (!nm)
            continue;
        n += snprintf(formats + n, sizeof(formats) - n, "%s%s",
                      n ? "+" : "", nm);
        if (n >= (int)sizeof(formats) - 1)
            break;
    }
    if (!formats[0])
        snprintf(formats, sizeof(formats), "none");

    printf("DEV %d %s %u %s\n",
           (id == (UINT_PTR)WAVE_MAPPER) ? -1 : (int)id,
           name, caps->wChannels, formats);
}

static int do_list(void)
{
    WAVEOUTCAPSA caps;
    UINT n = waveOutGetNumDevs();
    UINT i;
    MMRESULT mr;

    /* Device id -1 is WAVE_MAPPER; print it first so scripts can pick either. */
    mr = waveOutGetDevCapsA((UINT_PTR)WAVE_MAPPER, &caps, sizeof(caps));
    if (mr == MMSYSERR_NOERROR) {
        if (!quiet)
            print_dev((UINT_PTR)WAVE_MAPPER, &caps);
    } else {
        return fail("devcaps", mr);
    }

    for (i = 0; i < n; i++) {
        mr = waveOutGetDevCapsA(i, &caps, sizeof(caps));
        if (mr != MMSYSERR_NOERROR)
            return fail("devcaps", mr);
        if (!quiet)
            print_dev(i, &caps);
    }
    return 0;
}

static unsigned rd16(const unsigned char *p)
{
    return (unsigned)p[0] | ((unsigned)p[1] << 8);
}

static unsigned rd32(const unsigned char *p)
{
    return (unsigned)p[0] | ((unsigned)p[1] << 8) |
           ((unsigned)p[2] << 16) | ((unsigned)p[3] << 24);
}

/*
 * Parse a minimal RIFF/WAVE file: walk chunks looking for "fmt " and "data".
 * Only WAVE_FORMAT_PCM with 16-bit samples is accepted (the format this
 * test tool needs; anything else is reported as WAVERR_BADFORMAT).
 */
static int load_wav(const char *path, unsigned *rate, unsigned *channels,
                    unsigned *bits, unsigned char **pcm, unsigned *nbytes)
{
    FILE *f;
    unsigned char hdr[12], ch[8], fmt[16];
    unsigned chunk_sz, data_sz = 0;
    long data_off = 0, fsz;
    unsigned tag;
    int have_fmt = 0;

    f = fopen(path, "rb");
    if (!f)
        return fail("wav_open", MMSYSERR_ERROR);

    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        return fail("wav_seek", MMSYSERR_ERROR);
    }
    fsz = ftell(f);
    if (fsz < 44 || fseek(f, 0, SEEK_SET) != 0) {
        fclose(f);
        return fail("wav_size", MMSYSERR_ERROR);
    }

    if (fread(hdr, 1, 12, f) != 12 ||
        memcmp(hdr, "RIFF", 4) != 0 || memcmp(hdr + 8, "WAVE", 4) != 0) {
        fclose(f);
        return fail("wav_riff", WAVERR_BADFORMAT);
    }

    while (ftell(f) + 8 <= fsz && fread(ch, 1, 8, f) == 8) {
        chunk_sz = rd32(ch + 4);
        tag = rd32(ch);

        if (tag == rd32((const unsigned char *)"fmt ")) {
            if (chunk_sz < 16 || fread(fmt, 1, 16, f) != 16) {
                fclose(f);
                return fail("wav_fmt", WAVERR_BADFORMAT);
            }
            have_fmt = 1;
            /* Pad byte after odd-sized chunks. */
            if (fseek(f, (chunk_sz - 16) + (chunk_sz & 1), SEEK_CUR) != 0)
                break;
        } else if (tag == rd32((const unsigned char *)"data")) {
            data_off = ftell(f);
            data_sz = chunk_sz;
            if (fseek(f, chunk_sz + (chunk_sz & 1), SEEK_CUR) != 0)
                break;
        } else {
            if (fseek(f, chunk_sz + (chunk_sz & 1), SEEK_CUR) != 0)
                break;
        }
    }

    if (!have_fmt || data_off == 0) {
        fclose(f);
        return fail("wav_chunks", WAVERR_BADFORMAT);
    }
    if (rd16(fmt) != WAVE_FORMAT_PCM || rd16(fmt + 14) != 16) {
        fclose(f);
        return fail("wav_format", WAVERR_BADFORMAT);
    }
    if (data_sz == 0 || data_sz > MAX_PCM_BYTES ||
        (long)data_sz > fsz - data_off) {
        fclose(f);
        return fail("wav_databytes", WAVERR_BADFORMAT);
    }

    *pcm = (unsigned char *)malloc(data_sz);
    if (!*pcm) {
        fclose(f);
        return fail("pcm_alloc", MMSYSERR_NOMEM);
    }
    if (fseek(f, data_off, SEEK_SET) != 0 ||
        fread(*pcm, 1, data_sz, f) != data_sz) {
        free(*pcm);
        *pcm = NULL;
        fclose(f);
        return fail("wav_read", MMSYSERR_ERROR);
    }
    fclose(f);

    *channels = rd16(fmt + 2);
    *rate = rd32(fmt + 4);
    *bits = 16;
    *nbytes = data_sz;
    return 0;
}

/* Generate a sine of `seconds` on every channel; 8-bit samples are unsigned. */
static int gen_sine(double freq, unsigned seconds, unsigned rate,
                    unsigned channels, unsigned bits,
                    unsigned char **pcm, unsigned *nbytes)
{
    unsigned long frames = (unsigned long)rate * seconds;
    unsigned bps = bits / 8;
    unsigned long total = frames * channels * bps;
    unsigned long i;
    unsigned ch;
    unsigned char *p;

    if (total == 0 || total > MAX_PCM_BYTES)
        return fail("gen_size", MMSYSERR_INVALPARAM);

    p = (unsigned char *)malloc(total);
    if (!p)
        return fail("pcm_alloc", MMSYSERR_NOMEM);

    for (i = 0; i < frames; i++) {
        double t = (double)i / (double)rate;
        double s = sin(2.0 * M_PI * freq * t);
        for (ch = 0; ch < channels; ch++) {
            unsigned char *d = p + (i * channels + ch) * bps;
            if (bits == 16) {
                short v = (short)(s * 32767.0);
                d[0] = (unsigned char)(v & 0xff);
                d[1] = (unsigned char)((v >> 8) & 0xff);
            } else {
                unsigned char v = (unsigned char)(128.0 + s * 127.0);
                d[0] = v;
            }
        }
    }
    *pcm = p;
    *nbytes = (unsigned)total;
    return 0;
}

typedef struct {
    WAVEHDR wh;
    int in_use;
} Slot;

static void cleanup(HWAVEOUT hwo, Slot *slots, unsigned char *pcm)
{
    int i;
    if (hwo) {
        /* Reset dequeues any still-queued headers so unprepare can run. */
        waveOutReset(hwo);
        for (i = 0; i < (int)nbufs; i++)
            if (slots[i].wh.dwFlags & WHDR_PREPARED)
                waveOutUnprepareHeader(hwo, &slots[i].wh, sizeof(WAVEHDR));
        waveOutClose(hwo);
    }
    free(pcm);
}

static int play(HWAVEOUT hwo, const unsigned char *pcm, unsigned nbytes,
                unsigned rate, unsigned channels, unsigned bits, Slot *slots)
{
    unsigned block_align = channels * (bits / 8);
    unsigned chunk = (unsigned)((unsigned long)rate * chunk_ms / 1000u * block_align);
    unsigned next = 0, submitted = 0, finished = 0;
    DWORD t0;
    MMTIME mt;
    unsigned long played = nbytes;
    unsigned ms;
    MMRESULT mr;
    int i, rc = 0;

    if (chunk < block_align)
        chunk = block_align;
    chunk -= chunk % block_align;

    t0 = GetTickCount();

    while (finished < nbytes) {
        /* Recycle any finished slot for the next chunk. */
        for (i = 0; i < (int)nbufs && next < nbytes; i++) {
            if (slots[i].in_use && !(slots[i].wh.dwFlags & WHDR_DONE))
                continue;
            if (slots[i].in_use) {
                mr = waveOutUnprepareHeader(hwo, &slots[i].wh, sizeof(WAVEHDR));
                if (mr != MMSYSERR_NOERROR) {
                    rc = fail("unprepare", mr);
                    goto out;
                }
                finished += slots[i].wh.dwBufferLength;
                slots[i].in_use = 0;
            }

            {
                unsigned n = (next + chunk <= nbytes) ? chunk : (nbytes - next);
                memset(&slots[i].wh, 0, sizeof(WAVEHDR));
                /* lpData points into the PCM buffer; it must outlive the header. */
                slots[i].wh.lpData = (LPSTR)(pcm + next);
                slots[i].wh.dwBufferLength = n;
                mr = waveOutPrepareHeader(hwo, &slots[i].wh, sizeof(WAVEHDR));
                if (mr != MMSYSERR_NOERROR) {
                    rc = fail("prepare", mr);
                    goto out;
                }
                mr = waveOutWrite(hwo, &slots[i].wh, sizeof(WAVEHDR));
                if (mr != MMSYSERR_NOERROR) {
                    rc = fail("write", mr);
                    goto out;
                }
                if (!quiet)
                    printf("PLAY chunk=%u bytes=%u\n", submitted, n);
                slots[i].in_use = 1;
                next += n;
                submitted++;
            }
        }

        if (next >= nbytes) {
            int active = 0;
            for (i = 0; i < (int)nbufs; i++)
                if (slots[i].in_use && !(slots[i].wh.dwFlags & WHDR_DONE))
                    active++;
            if (!active)
                break;
        }
        if (done_event)
            WaitForSingleObject(done_event, 100);
        else
            Sleep(2);
    }

    /* Drain: unprepare the last headers that are done but not yet recycled. */
    for (i = 0; i < (int)nbufs; i++) {
        if (!slots[i].in_use)
            continue;
        mr = waveOutUnprepareHeader(hwo, &slots[i].wh, sizeof(WAVEHDR));
        if (mr != MMSYSERR_NOERROR) {
            rc = fail("unprepare", mr);
            goto out;
        }
        slots[i].in_use = 0;
    }

    ms = GetTickCount() - t0;

    mt.wType = TIME_BYTES;
    if (waveOutGetPosition(hwo, &mt, sizeof(mt)) == MMSYSERR_NOERROR) {
        if (mt.wType == TIME_BYTES)
            played = mt.u.cb;
        else if (mt.wType == TIME_SAMPLES)
            played = (unsigned long)mt.u.sample * block_align;
        else if (mt.wType == TIME_MS)
            ms = mt.u.ms;
    }

    if (!quiet)
        printf("DONE played=%lu ms=%u\n", played, ms);
    fflush(stdout);

out:
    return rc;
}

static int parse_uint(const char *s, unsigned *out)
{
    char *end;
    unsigned long v;
    if (!s || !*s)
        return -1;
    v = strtoul(s, &end, 10);
    if (*end || v > 0x7fffffffUL)
        return -1;
    *out = (unsigned)v;
    return 0;
}

/* NtQueryTimerResolution: the clock tick, in 100 ns units. */
static int do_timer(void)
{
    typedef LONG (WINAPI *QTR)(PULONG, PULONG, PULONG);
    QTR q = (QTR)GetProcAddress(GetModuleHandleA("ntdll.dll"), "NtQueryTimerResolution");
    ULONG lo = 0, hi = 0, cur = 0;

    if (!q || q(&lo, &hi, &cur) < 0)
        return fail("timer", MMSYSERR_ERROR);
    /* NT names them backwards: "maximum" is the finest. */
    printf("TIMER min=%lu max=%lu cur=%lu\n", lo, hi, cur);
    fflush(stdout);
    return 0;
}

static void usage(void)
{
    fprintf(stderr,
            "usage: testplay [-s N] [-r N] [-c N] [-b N] [-f FREQ] [-w FILE] [-n N] [-m MS]\n"
            "                [-p] [-t] [-T] [-l] [-q]\n"
            "  -s N    seconds (default %d)\n"
            "  -r N    sample rate (default %d)\n"
            "  -c N    channels (default %d)\n"
            "  -b N    bits per sample 8|16 (default %d)\n"
            "  -f FREQ sine Hz (default %g)\n"
            "  -w FILE play PCM 16-bit WAV instead of a sine\n"
            "  -n N    buffers kept queued (default %d, 2..32)\n"
            "  -m MS   milliseconds per buffer (default %d, 5..1000)\n"
            "  -p      poll for finished buffers with Sleep(2) (default: wait on an event)\n"
            "  -t      timeBeginPeriod(1) while playing\n"
            "  -T      print the clock tick and exit\n"
            "  -l      list waveOut devices and exit\n"
            "  -q      quiet: print only on failure\n",
            DEF_SECONDS, DEF_RATE, DEF_CHANNELS, DEF_BITS, DEF_FREQ,
            DEF_NBUFS, DEF_CHUNK_MS);
}

int main(int argc, char **argv)
{
    unsigned seconds = DEF_SECONDS;
    unsigned rate = DEF_RATE;
    unsigned channels = DEF_CHANNELS;
    unsigned bits = DEF_BITS;
    double freq = DEF_FREQ;
    const char *wav_path = NULL;
    int list_mode = 0;
    int event_mode = 1, fine_timer = 0;
    unsigned char *pcm = NULL;
    unsigned nbytes = 0;
    HWAVEOUT hwo = NULL;
    WAVEFORMATEX wfx;
    Slot slots[MAX_NBUFS];
    MMRESULT mr;
    int i, rc = 0;

    memset(slots, 0, sizeof(slots));

    for (i = 1; i < argc; i++) {
        const char *a = argv[i];
        const char *v = NULL;

        if (a[0] != '-' || !a[1]) {
            usage();
            return fail("usage", MMSYSERR_INVALPARAM);
        }
        if (a[1] == 'l' && !a[2]) { list_mode = 1; continue; }
        if (a[1] == 'q' && !a[2]) { quiet = 1; continue; }
        if (a[1] == 'p' && !a[2]) { event_mode = 0; continue; }
        if (a[1] == 't' && !a[2]) { fine_timer = 1; continue; }
        if (a[1] == 'T' && !a[2]) return do_timer();
        if (a[1] == 'h' && !a[2]) { usage(); return 0; }

        /* -x VALUE or -xVALUE */
        if (a[2])
            v = a + 2;
        else if (i + 1 < argc)
            v = argv[++i];
        else {
            usage();
            return fail("usage", MMSYSERR_INVALPARAM);
        }

        switch (a[1]) {
        case 's':
            if (parse_uint(v, &seconds) || seconds == 0) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        case 'r':
            if (parse_uint(v, &rate) || rate < 1000 || rate > 384000) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        case 'c':
            if (parse_uint(v, &channels) || channels < 1 || channels > 8) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        case 'b':
            if (parse_uint(v, &bits) || (bits != 8 && bits != 16)) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        case 'f':
            freq = atof(v);
            if (freq <= 0.0) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        case 'w':
            wav_path = v;
            break;
        case 'n':
            if (parse_uint(v, &nbufs) || nbufs < 2 || nbufs > MAX_NBUFS) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        case 'm':
            if (parse_uint(v, &chunk_ms) || chunk_ms < 5 || chunk_ms > 1000) {
                usage();
                return fail("usage", MMSYSERR_INVALPARAM);
            }
            break;
        default:
            usage();
            return fail("usage", MMSYSERR_INVALPARAM);
        }
    }

    if (list_mode)
        return do_list();

    if (wav_path) {
        rc = load_wav(wav_path, &rate, &channels, &bits, &pcm, &nbytes);
        if (rc)
            return rc;
    } else {
        rc = gen_sine(freq, seconds, rate, channels, bits, &pcm, &nbytes);
        if (rc)
            return rc;
    }

    memset(&wfx, 0, sizeof(wfx));
    wfx.wFormatTag = WAVE_FORMAT_PCM;
    wfx.nChannels = (WORD)channels;
    wfx.nSamplesPerSec = rate;
    wfx.wBitsPerSample = (WORD)bits;
    wfx.nBlockAlign = (WORD)(channels * (bits / 8));
    wfx.nAvgBytesPerSec = rate * wfx.nBlockAlign;
    wfx.cbSize = 0;

    /* WAVE_MAPPER picks the default device; without an event (-p) we poll. */
    if (event_mode)
        done_event = CreateEventA(NULL, FALSE, FALSE, NULL);
    if (fine_timer)
        timeBeginPeriod(1);
    mr = waveOutOpen(&hwo, (UINT_PTR)WAVE_MAPPER, &wfx,
                     (DWORD_PTR)done_event, 0,
                     done_event ? CALLBACK_EVENT : CALLBACK_NULL);
    if (mr != MMSYSERR_NOERROR) {
        rc = fail("open", mr);
        cleanup(hwo, slots, pcm);
        return rc;
    }

    if (!quiet)
        printf("OPEN %u %u %u %u\n", rate, channels, bits, nbytes);
    fflush(stdout);

    rc = play(hwo, pcm, nbytes, rate, channels, bits, slots);
    cleanup(hwo, slots, pcm);
    if (fine_timer)
        timeEndPeriod(1);
    return rc;
}
