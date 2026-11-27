// HALOT BOX cxdlp.dll LUT dumper -> lut_range.bin + lut_bits.bin + lut_meta.txt
// Build: wineg++ -m64 -O1 -o lutdump2.exe lutdump2.c
// Run: wine ./lutdump2.exe && wineserver -k
#include <windows.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>

#define LOG(...) do { fprintf(stderr, __VA_ARGS__); fflush(stderr); } while(0)

typedef void* (__attribute__((ms_abi)) *ctor_fn)(void*);
typedef void (__attribute__((ms_abi)) *init_fn)(void*, int64_t,int64_t,int64_t,
    uint32_t,uint32_t,uint32_t, void*,void*,void*, uint32_t,int64_t,uint32_t,uint32_t);
typedef int (__attribute__((ms_abi)) *ensureBeadLut_fn)(void*);  // returns bool (int)

/* MSVC x64 std::vector layout as passed by value on the stack: {first,last,end} */
typedef struct { uint32_t* f; uint32_t* l; uint32_t* e; } VecU32;
typedef struct { float* f; float* l; float* e; } VecF32;

static HMODULE hmod;
static FARPROC getord(int ord) {
    FARPROC p = GetProcAddress(hmod, (LPCSTR)(intptr_t)ord);
    if (!p) { LOG("FAILED ordinal %d\n", ord); exit(1); }
    return p;
}

int main(void) {
    const char* dllpath = "Z:\\run\\media\\makzumi\\storage\\Games\\halot-box\\drive_c\\Program Files\\HALOT BOX\\cxdlp.dll";
    hmod = LoadLibraryA(dllpath);
    if (!hmod) hmod = LoadLibraryA("cxdlp.dll");
    if (!hmod) { LOG("LoadLibrary failed err=%lu\n", GetLastError()); return 1; }
    LOG("Loaded @ %p\n", (void*)hmod);

    ctor_fn        ctor           = (ctor_fn)getord(61);
    init_fn        init           = (init_fn)getord(371);
    ensureBeadLut_fn ensureBeadLut = (ensureBeadLut_fn)getord(310);

    void* obj = calloc(1, 0x10000);
    ctor(obj);

    uint32_t cols[2] = {29, 28};
    VecU32 vcols = { cols, cols + 2, cols + 2 };
    float s3 = 1.7320508075688772f, one = 1.0f;
    VecF32 vs3 = { &s3, &s3 + 1, &s3 + 1 };
    VecF32 vo  = { &one, &one + 1, &one + 1 };
    int64_t P1=100000, P2=21168000, P3=11837000, P11=470053;
    uint32_t P4=15120, P5=6230, P6=19, P10=6, P12=120, P13=90;
    init(obj, P1,P2,P3,P4,P5,P6, &vcols,&vs3,&vo, P10,P11,P12,P13);
    LOG("init done\n");

    int ok = ensureBeadLut(obj);
    LOG("ensureBeadLut: %d\n", ok);
    if (!ok) { LOG("ensureBeadLut failed\n"); return 1; }

    // obj is a struct; first member at +0 is the LUT pointer
    void* lut = *(void**)obj;
    LOG("LUT ptr: %p\n", lut);

    // this->0x250 = max bit idx (542)
    // this->0x260 = cols (15120)
    // this->0x264 = rows (6230)
    uint32_t max_bit = *(uint32_t*)((char*)obj + 0x250);
    uint32_t ncols   = *(uint32_t*)((char*)obj + 0x260);
    uint32_t nrows   = *(uint32_t*)((char*)obj + 0x264);
    LOG("obj->0x250 max_bit=%u, 0x260 cols=%u, 0x264 rows=%u\n", max_bit, ncols, nrows);

    // LUT->0x3f8 = ebp (64)
    // LUT->0x3fc = edi (237)
    // LUT->0x400 = enable flag
    uint32_t ebp = *(uint32_t*)((char*)lut + 0x3f8);
    uint32_t edi = *(uint32_t*)((char*)lut + 0x3fc);
    uint8_t  en  = *(uint8_t*) ((char*)lut + 0x400);
    LOG("LUT->0x3f8 ebp=%u, 0x3fc edi=%u, 0x400 en=%u\n", ebp, edi, en);

    // LUT->0x3c8 = range array pointer (uint32_t*), length = ebp * nrows + 1
    // LUT->0x3e0 = bit index array pointer (uint32_t*), length = range[ebp*nrows]
    uint32_t* range_arr = *(uint32_t**)((char*)lut + 0x3c8);
    uint32_t* bits_arr  = *(uint32_t**)((char*)lut + 0x3e0);
    size_t ncells = (size_t)ebp * nrows;
    uint32_t total_bits = range_arr[ncells];
    LOG("ncells=%zu, total_bits=%u\n", ncells, total_bits);

    // Dump range array
    FILE* fr = fopen("lut_range.bin", "wb");
    if (!fr) { LOG("fopen lut_range.bin failed\n"); return 1; }
    fwrite(range_arr, sizeof(uint32_t), ncells + 1, fr);
    fclose(fr);
    LOG("wrote lut_range.bin: %zu u32\n", ncells + 1);

    // Dump bits array
    FILE* fb = fopen("lut_bits.bin", "wb");
    if (!fb) { LOG("fopen lut_bits.bin failed\n"); return 1; }
    fwrite(bits_arr, sizeof(uint32_t), total_bits, fb);
    fclose(fb);
    LOG("wrote lut_bits.bin: %u u32\n", total_bits);

    // Dump meta
    FILE* fm = fopen("lut_meta.txt", "w");
    if (!fm) { LOG("fopen lut_meta.txt failed\n"); return 1; }
    fprintf(fm, "%u %u %u %zu %u\n", ebp, edi, nrows, ncells, total_bits);
    fclose(fm);
    LOG("wrote lut_meta.txt\n");

    LOG("DONE\n");
    return 0;
}