// [FNC-MENU] PC "DISPLAY" tab in the game's own Settings screen.
//
// The UI side (new category + rows) is added to the Apt movie gamesettingdata
// inside dist\game\databigs\xenon\fe.big by tools\display_tab\aptpatch_display.py
// (run by setup). Without that patch the tab is simply not there.
// The game's Settings screen reads/writes every row through the native Apt
// object ION_Settings. Its three methods are plain recompiled functions:
//   sub_82E64098 GetDefaultSettingValue(category, setting)        -> mgr vtable +4
//   sub_82E64120 GetCurrentSettingValue(category, setting)        -> mgr vtable +8
//   sub_82E641A8 SetCurrentSettingValue(category, setting, value) -> mgr vtable +12
// (registered by sub_82DEE870; mgr = *(u32*)0x83050E70). For our category id
// (kCategory) we answer from / write to the runtime cvars and FNCReborn.toml.
//
// Returning a number to Apt: the game has no tiny "make int" helper we know of,
// so we borrow a real int-valued setting ("donor", learned from the game's own
// calls) through the manager, check the returned Apt value is a fresh int
// (type 7 in the low 7 bits of +4, value at +8) and overwrite its value.

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <mutex>
#include <regex>
#include <sstream>
#include <string>

#include <rex/cvar.h>
#include <rex/logging.h>
#include <rex/ppc.h>
#include <rex/ppc/func.h>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#endif


extern "C" {
void __imp__sub_82E64098(PPCContext& __restrict ctx, uint8_t* base);
void __imp__sub_82E64120(PPCContext& __restrict ctx, uint8_t* base);
void __imp__sub_82E641A8(PPCContext& __restrict ctx, uint8_t* base);
void sub_825B1908(PPCContext& __restrict ctx, uint8_t* base);  // Apt arg(i) -> value*
void sub_825AE3F8(PPCContext& __restrict ctx, uint8_t* base);  // value* -> int
void sub_823B0BA8(PPCContext& __restrict ctx, uint8_t* base);  // holder init
void sub_82B40288(PPCContext& __restrict ctx, uint8_t* base);  // holder -> value*
void sub_82833050(PPCContext& __restrict ctx, uint8_t* base);  // holder2 = value
void sub_82833110(PPCContext& __restrict ctx, uint8_t* base);  // set-call result
}

namespace {

constexpr uint32_t kCategory = 200;     // must match aptpatch_display.py
constexpr uint32_t kFirstSetting = 201;
constexpr uint32_t kMgrPtr = 0x83050E70;  // lis r11,-31995 (0x8305) + 3696
constexpr uint32_t kHolderGet = 0x82F9E0BC;
constexpr uint32_t kHolderSet = 0x82F9E0C4;

struct Row {
  const char* cvar;
  const char* values[7];  // cvar text per option index
  int count;
  int def;                // default option index
  bool restart;
};

// Order = setting ids 201, 202, ... (same order as the labels in the Apt patch).
const Row kRows[] = {
    {"fnc_frame_rate", {"30", "60", "120", "144", "165", "180", "240"}, 7, 1, true},
    {"vsync", {"false", "true"}, 2, 1, false},
    {"resolution_scale", {"1", "2", "3"}, 3, 0, true},
    {"swap_post_effect", {"none", "fxaa", "fxaa_extreme"}, 3, 0, false},
    {"fullscreen", {"false", "true"}, 2, 1, false},
};
constexpr uint32_t kRowCount = sizeof(kRows) / sizeof(kRows[0]);

inline uint8_t* Host(uint8_t* base, uint32_t a) {
  return base + a + (a >= 0xE0000000u ? 0x1000u : 0u);
}
inline uint32_t Ld(uint8_t* base, uint32_t a) {
  return __builtin_bswap32(*reinterpret_cast<volatile uint32_t*>(Host(base, a)));
}
inline void St(uint8_t* base, uint32_t a, uint32_t v) {
  *reinterpret_cast<volatile uint32_t*>(Host(base, a)) = __builtin_bswap32(v);
}

uint32_t ArgPtr(PPCContext& ctx, uint8_t* base, int i) {
  ctx.r3.u64 = uint64_t(i);
  sub_825B1908(ctx, base);
  return ctx.r3.u32;
}
int32_t ArgInt(PPCContext& ctx, uint8_t* base, int i) {
  ctx.r3.u64 = ArgPtr(ctx, base, i);
  sub_825AE3F8(ctx, base);
  return int32_t(ctx.r3.u32);
}

bool IsOurs(int32_t a0, int32_t a1) { return a0 == int32_t(kCategory) || a1 == int32_t(kCategory); }
int32_t SettingOf(int32_t a0, int32_t a1) { return a0 == int32_t(kCategory) ? a1 : a0; }
const Row* RowOf(int32_t setting) {
  uint32_t i = uint32_t(setting) - kFirstSetting;
  return i < kRowCount ? &kRows[i] : nullptr;
}

int CurrentIndex(const Row& r) {
  std::string cur = rex::cvar::GetFlagByName(r.cvar);
  if (std::string(r.cvar) == "fnc_frame_rate") {  // double: pick the nearest option
    double v = std::atof(cur.c_str());
    int best = r.def;
    double bd = 1e9;
    for (int i = 0; i < r.count; ++i) {
      double d = std::fabs(std::atof(r.values[i]) - v);
      if (d < bd) { bd = d; best = i; }
    }
    return best;
  }
  for (int i = 0; i < r.count; ++i) {
    if (cur == r.values[i]) return i;
  }
  return r.def;
}

std::filesystem::path TomlPath() {
#ifdef _WIN32
  wchar_t buf[MAX_PATH];
  DWORD n = GetModuleFileNameW(nullptr, buf, MAX_PATH);
  return std::filesystem::path(std::wstring(buf, n)).parent_path() / "FNCReborn.toml";
#else
  return "FNCReborn.toml";
#endif
}

// Rewrites (or appends) one `key = value` line in FNCReborn.toml, keeping everything else.
void SaveToToml(const char* key, const std::string& value, bool is_string) {
  auto path = TomlPath();
  std::string text;
  {
    std::ifstream in(path, std::ios::binary);
    std::stringstream ss;
    ss << in.rdbuf();
    text = ss.str();
  }
  std::string val = is_string ? "\"" + value + "\"" : value;
  std::regex re(std::string("(^|\\n)[ \\t]*") + key + "[ \\t]*=[^\\r\\n]*");
  std::string line = std::string(key) + " = " + val;
  if (std::regex_search(text, re)) {
    text = std::regex_replace(text, re, "$1" + line, std::regex_constants::format_first_only);
  } else {
    if (!text.empty() && text.back() != '\n') text += "\n";
    text += line + "\n";
  }
  std::ofstream out(path, std::ios::binary | std::ios::trunc);
  out << text;
}

void Apply(const Row& r, int index) {
  if (index < 0 || index >= r.count) return;
  const char* v = r.values[index];
  bool ok = rex::cvar::SetFlagByName(r.cvar, v);
  bool is_string = std::string(r.cvar) == "swap_post_effect";
  SaveToToml(r.cvar, v, is_string);
  REXLOG_INFO("[FNC-MENU] {} = {} (set {}, saved to FNCReborn.toml{})", r.cvar, v, ok ? "ok" : "FAILED",
              r.restart ? ", takes effect after restart" : "");
}

// A real (category, setting) pair whose value comes back as an Apt int.
std::mutex g_mu;
bool g_have_donor = false;
uint32_t g_donor_a0 = 0, g_donor_a1 = 0;
int g_logged = 0;

// Calls mgr->vtable[slot](mgr, a0, a1, holder) like the original handlers and
// returns the Apt value pointer it produced.
uint32_t CallMgr(PPCContext& ctx, uint8_t* base, uint32_t slot_off, uint32_t a0, uint32_t a1) {
  ctx.r3.u64 = kHolderGet;
  sub_823B0BA8(ctx, base);
  uint32_t mgr = Ld(base, kMgrPtr);
  uint32_t fn = Ld(base, Ld(base, mgr) + slot_off);
  ctx.r3.u64 = mgr;
  ctx.r4.u64 = a0;
  ctx.r5.u64 = a1;
  ctx.r6.u64 = kHolderGet;
  rex::runtime::ResolveIndirectFunction(fn)(ctx, base);
  return Ld(base, kHolderGet + 4);
}

int AptType(uint8_t* base, uint32_t v) {
  if (!v) return -1;
  uint32_t flags = Ld(base, v + 4);
  return int(int8_t(uint8_t(flags << 1)) >> 1);  // low 7 bits, sign-extended
}

void LearnDonor(uint8_t* base, int32_t a0, int32_t a1) {
  uint32_t v = Ld(base, kHolderGet + 4);
  int t = AptType(base, v);
  std::lock_guard lk(g_mu);
  if (g_logged < 40) {
    ++g_logged;
    REXLOG_INFO("[FNC-MENU] game get({}, {}) -> value {:08X} type {} raw {:08X} {:08X} {:08X}", a0, a1, v, t,
                v ? Ld(base, v) : 0, v ? Ld(base, v + 4) : 0, v ? Ld(base, v + 8) : 0);
  }
  if (!g_have_donor && t == 7) {
    g_have_donor = true;
    g_donor_a0 = uint32_t(a0);
    g_donor_a1 = uint32_t(a1);
    REXLOG_INFO("[FNC-MENU] donor = ({}, {})", a0, a1);
  }
}

// Builds the return value for our rows. Returns false if it can't (no donor yet,
// or the manager hands out a shared value we must not modify).
bool ReturnInt(PPCContext& ctx, uint8_t* base, uint32_t slot_off, int32_t number) {
  uint32_t a0, a1;
  {
    std::lock_guard lk(g_mu);
    if (!g_have_donor) return false;
    a0 = g_donor_a0;
    a1 = g_donor_a1;
  }
  uint32_t first = CallMgr(ctx, base, slot_off, a0, a1);
  uint32_t v = CallMgr(ctx, base, slot_off, a0, a1);
  if (!v || AptType(base, v) != 7 || v == first) {
    static int warned = 0;
    if (warned++ < 5) {
      REXLOG_WARN("[FNC-MENU] cannot return a number: value {:08X} type {} (first {:08X})", v,
                  AptType(base, v), first);
    }
    return false;
  }
  St(base, v + 8, uint32_t(number));
  ctx.r3.u64 = kHolderGet;
  sub_82B40288(ctx, base);
  return true;
}

void HandleGet(PPCContext& ctx, uint8_t* base, uint32_t slot_off, bool is_default,
               void (*orig)(PPCContext&, uint8_t*)) {
  int32_t a0 = ArgInt(ctx, base, 0);
  int32_t a1 = ArgInt(ctx, base, 1);
  if (!IsOurs(a0, a1)) {
    orig(ctx, base);
    uint32_t ret = ctx.r3.u32;
    if (!is_default) LearnDonor(base, a0, a1);
    ctx.r3.u64 = ret;
    return;
  }
  const Row* r = RowOf(SettingOf(a0, a1));
  int idx = r ? (is_default ? r->def : CurrentIndex(*r)) : 0;
  REXLOG_INFO("[FNC-MENU] {}({}, {}) -> {}", is_default ? "GetDefault" : "GetCurrent", a0, a1, idx);
  if (!ReturnInt(ctx, base, slot_off, idx)) {
    ctx.r3.u64 = kHolderGet;  // empty holder -> undefined; the row shows its first option
    sub_823B0BA8(ctx, base);
    ctx.r3.u64 = kHolderGet;
    sub_82B40288(ctx, base);
  }
}

}  // namespace

extern "C" void sub_82E64098(PPCContext& __restrict ctx, uint8_t* base) {
  HandleGet(ctx, base, 4, true, __imp__sub_82E64098);
}

extern "C" void sub_82E64120(PPCContext& __restrict ctx, uint8_t* base) {
  HandleGet(ctx, base, 8, false, __imp__sub_82E64120);
}

extern "C" void sub_82E641A8(PPCContext& __restrict ctx, uint8_t* base) {
  int32_t a0 = ArgInt(ctx, base, 0);
  int32_t a1 = ArgInt(ctx, base, 1);
  if (!IsOurs(a0, a1)) {
    __imp__sub_82E641A8(ctx, base);
    return;
  }
  uint32_t vptr = ArgPtr(ctx, base, 2);
  ctx.r3.u64 = vptr;
  sub_825AE3F8(ctx, base);
  int32_t index = int32_t(ctx.r3.u32);
  const Row* r = RowOf(SettingOf(a0, a1));
  REXLOG_INFO("[FNC-MENU] SetCurrent({}, {}, {})", a0, a1, index);
  if (r) Apply(*r, index);
  ctx.r3.u64 = kHolderSet;
  ctx.r4.u64 = vptr;
  sub_82833050(ctx, base);
  ctx.r3.u64 = kHolderSet;
  sub_82833110(ctx, base);
}
