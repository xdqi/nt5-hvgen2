/* Minimal fixes so mingw-w64 ddk/portcls.h compiles freestanding.
 * C++-only bits: DECLSPEC_NOVTABLE / DECLSPEC_NOTHROW (windef.h skips
 * winnt.h once NT_INCLUDED is set by ntddk.h).
 * C and C++: TCHAR (ksmedia.h) and KSRTAUDIO_HW* (used by portcls.h
 * WaveRT macros but never defined in this tree).
 */
#ifndef CXX_SMOKE_DDK_COMPAT_H
#define CXX_SMOKE_DDK_COMPAT_H

#ifndef DECLSPEC_NOVTABLE
#define DECLSPEC_NOVTABLE
#endif
#ifndef DECLSPEC_NOTHROW
#define DECLSPEC_NOTHROW __declspec(nothrow)
#endif

#ifndef _TCHAR_DEFINED
#define _TCHAR_DEFINED
typedef char TCHAR;
#endif

/* WDK ksmedia.h shapes, unused by this smoke test beyond completing
 * IMiniportWaveRT* declarations. */
#ifndef KSRTAUDIO_HWLATENCY_DEFINED
#define KSRTAUDIO_HWLATENCY_DEFINED
typedef struct tagKSRTAUDIO_HWLATENCY {
    ULONG ChipsetDelay;
    ULONG CodecDelay;
} KSRTAUDIO_HWLATENCY;

typedef struct tagKSRTAUDIO_HWREGISTER {
    PVOID Register;
    ULONG Width;
    ULONG Numerator;
    ULONG Denominator;
} KSRTAUDIO_HWREGISTER;
#endif

#endif /* CXX_SMOKE_DDK_COMPAT_H */
