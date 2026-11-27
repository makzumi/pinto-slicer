// HALOT BOX cxdlp.dll geometry dumper -> lampmask.txt + beaddata.txt
// Build: wineg++ -m64 -O1 -o dump2.exe dump2.c
// Run: wine ./dump2.exe && wineserver -k
#include <windows.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>

#define LOG(...) do { fprintf(stderr, __VA_ARGS__); fflush(stderr); } while(0)

typedef struct { uint32_t* f; uint32_t* l; uint32_t* e; } VecU32;
typedef struct { float* f; float* l; float* e; } VecF32;
typedef struct { int64_t a; int64_t b; } PairI64;
typedef struct { PairI64* f; PairI64* l; PairI64* e; } VecPair;
typedef struct { VecPair* f; VecPair* l; VecPair* e; } VecVecPair;
typedef struct { PairI64 a; PairI64 b; } Pair2xI64;
typedef struct { Pair2xI64* f; Pair2xI64* l; Pair2xI64* e; } VecPair2;
typedef struct { uint64_t ka; uint32_t kb; uint32_t pad; Pair2xI64* f; Pair2xI64* l; Pair2xI64* e; } BeadEntry;
typedef struct { BeadEntry* f; BeadEntry* l; BeadEntry* e; } VecBeadEntry;

typedef void* (__attribute__((ms_abi)) *ctor_fn)(void*);
typedef void (__attribute__((ms_abi)) *init_fn)(void*, int64_t,int64_t,int64_t,
    uint32_t,uint32_t,uint32_t, void*,void*,void*, uint32_t,int64_t,uint32_t,uint32_t);
typedef void (__attribute__((ms_abi)) *getLampMask_fn)(void*, void*);
typedef void (__attribute__((ms_abi)) *getbeaddata_fn)(void*, void*, int64_t, int64_t);

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

    ctor_fn        ctor        = (ctor_fn)getord(61);
    init_fn        init        = (init_fn)getord(371);
    getLampMask_fn getLampMask = (getLampMask_fn)getord(339);
    getbeaddata_fn getbeaddata = (getbeaddata_fn)getord(355);

    void* obj = calloc(1, 0x10000);
    ctor(obj);

    uint32_t cols[2] = {29, 28};
    VecU32 vcols = { cols, cols+2, cols+2 };
    float s3 = 1.7320508075688772f, one = 1.0f;
    VecF32 vs3 = { &s3, &s3+1, &s3+1 }, vo = { &one, &one+1, &one+1 };
    int64_t P1=100000, P2=21168000, P3=11837000, P11=470053;
    uint32_t P4=15120, P5=6230, P6=19, P10=6, P12=120, P13=90;
    init(obj, P1,P2,P3,P4,P5,P6, &vcols,&vs3,&vo, P10,P11,P12,P13);
    LOG("init done\n");

    // --- getLampMask -> lampmask.txt
    VecVecPair mask = {0};
    getLampMask(obj, &mask);
    size_t bands = (size_t)(mask.l - mask.f);
    FILE* f1 = fopen("lampmask.txt", "w");
    if (!f1) { LOG("no lampmask.txt\n"); return 1; }
    fprintf(f1, "%zu\n", bands);
    for (size_t i = 0; i < bands; i++) {
        VecPair* v = &mask.f[i];
        size_t cnt = (size_t)(v->l - v->f);
        fprintf(f1, "%zu %zu", i, cnt);
        for (size_t j = 0; j < cnt; j++)
            fprintf(f1, " %lld %lld", (long long)v->f[j].a, (long long)v->f[j].b);
        fprintf(f1, "\n");
    }
    fclose(f1);
    LOG("lampmask.txt: %zu bands\n", bands);

    // --- getbeaddata -> beaddata.txt
    VecBeadEntry bead = {0};
    getbeaddata(obj, &bead, P2, P3);
    size_t nbeads = (size_t)(bead.l - bead.f);
    FILE* f2 = fopen("beaddata.txt", "w");
    if (!f2) { LOG("no beaddata.txt\n"); return 1; }
    fprintf(f2, "%zu\n", nbeads);
    for (size_t i = 0; i < nbeads; i++) {
        BeadEntry* b = &bead.f[i];
        size_t cnt = (size_t)(b->l - b->f);
        fprintf(f2, "%zu %llu %u %zu", i, (unsigned long long)b->ka, b->kb, cnt);
        for (size_t j = 0; j < cnt; j++)
            fprintf(f2, " %lld %lld %lld %lld",
                (long long)b->f[j].a.a, (long long)b->f[j].a.b,
                (long long)b->f[j].b.a, (long long)b->f[j].b.b);
        fprintf(f2, "\n");
    }
    fclose(f2);
    LOG("beaddata.txt: %zu beads\n", nbeads);

    LOG("DONE\n");
    return 0;
}