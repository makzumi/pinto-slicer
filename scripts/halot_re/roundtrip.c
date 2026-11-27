// HALOT BOX cxdlp.dll round-trip test: call ord302 on decoded bitmap, dump mask
// Build: wineg++ -m64 -O1 -o roundtrip.exe roundtrip.c
// Run: wine ./roundtrip.exe && wineserver -k
#include <windows.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define LOG(...) do { fprintf(stderr, __VA_ARGS__); fflush(stderr); } while(0)

typedef struct { uint8_t* f; uint8_t* l; uint8_t* e; } VecU8;

typedef void* (__attribute__((ms_abi)) *ctor_fn)(void*);
typedef void (__attribute__((ms_abi)) *init_fn)(void*, int64_t,int64_t,int64_t,
    uint32_t,uint32_t,uint32_t, void*,void*,void*, uint32_t,int64_t,uint32_t,uint32_t);
// encodeCtbImage_16K_withNoAreaPartData_3bit (ord 302):
// static vector<u8> (sret, image, W, H, maskOut, beadsVec, uint, uint)
typedef void (__attribute__((ms_abi)) *encode3bit_fn)(void*, void*, uint32_t, uint32_t, void*, void*, uint32_t, uint32_t);

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

    ctor_fn      ctor      = (ctor_fn)getord(61);
    init_fn      init      = (init_fn)getord(371);
    encode3bit_fn encode3bit = (encode3bit_fn)getord(302);

    void* obj = calloc(1, 0x10000);
    ctor(obj);

    uint32_t cols[2] = {29, 28};
    uint32_t* vcols_f = cols; uint32_t* vcols_l = cols+2; uint32_t* vcols_e = cols+2;
    float s3 = 1.7320508075688772f, one = 1.0f;
    float* vs3_f = &s3; float* vs3_l = &s3+1; float* vs3_e = &s3+1;
    float* vo_f = &one; float* vo_l = &one+1; float* vo_e = &one+1;
    int64_t P1=100000, P2=21168000, P3=11837000, P11=470053;
    uint32_t P4=15120, P5=6230, P6=19, P10=6, P12=120, P13=90;
    init(obj, P1,P2,P3,P4,P5,P6, &vcols_f,&vs3_f,&vo_f, P10,P11,P12,P13);
    LOG("init done\n");

    // --- Load decoded layer 100 bitmap (binary 0/255, 6230x15120) ---
    const char* img_path = "Z:\\tmp\\halot_re\\layer_100.raw";
    FILE* fi = fopen(img_path, "rb");
    if (!fi) { LOG("fopen %s failed\n", img_path); return 1; }
    fseek(fi, 0, SEEK_END);
    long fsz = ftell(fi);
    fseek(fi, 0, SEEK_SET);
    const size_t COLS = 15120, ROWS = 6230;
    if (fsz != (long)(COLS * ROWS)) { LOG("size mismatch: %ld != %zu\n", fsz, COLS*ROWS); return 1; }
    uint8_t* image = malloc(COLS * ROWS);
    fread(image, 1, COLS * ROWS, fi);
    fclose(fi);
    LOG("loaded image: %zu bytes\n", COLS * ROWS);

    // --- Prepare output vectors ---
    VecU8 maskOut = {0}, beadsVec = {0};
    // Call static encode function:
    // RCX = sret (ignored for void return), RDX = image, R8D = W, R9D = H
    // stack: maskOut @ [rsp+0x120], beadsVec @ [rsp+0x128], p6=0, p7=0
    encode3bit(NULL, image, COLS, ROWS, &maskOut, &beadsVec, 0, 0);
    LOG("encode3bit done\n");

    size_t mask_len = maskOut.l - maskOut.f;
    LOG("maskOut length: %zu bytes\n", mask_len);

    // Dump mask
    FILE* fm = fopen("roundtrip_mask.bin", "wb");
    if (!fm) { LOG("fopen roundtrip_mask.bin failed\n"); return 1; }
    fwrite(maskOut.f, 1, mask_len, fm);
    fclose(fm);
    LOG("wrote roundtrip_mask.bin\n");

    // Popcount
    int pop = 0;
    for (size_t i = 0; i < mask_len; i++) pop += __builtin_popcount(maskOut.f[i]);
    LOG("mask popcount: %d\n", pop);

    // Also dump for comparison
    FILE* ft = fopen("roundtrip_mask.txt", "w");
    if (ft) {
        for (size_t i = 0; i < mask_len; i++)
            fprintf(ft, "%02x ", maskOut.f[i]);
        fprintf(ft, "\n");
        fclose(ft);
    }

    free(image);
    LOG("DONE\n");
    return 0;
}