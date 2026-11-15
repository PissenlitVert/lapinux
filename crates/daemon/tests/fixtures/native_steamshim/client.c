#define _GNU_SOURCE
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>

extern bool SteamAPI_Init(void);

#define CHECK(condition) do { if (!(condition)) { \
    fprintf(stderr, "CHECK failed at line %d: %s\n", __LINE__, #condition); return 1; \
} } while (0)

/* The SDK's accessor stores a function, counter and context pointer. */
struct context { void (*init)(void *); uintptr_t generation; void *object; };
typedef void *(*find_interface_fn)(int, const char *);
static find_interface_fn find_interface;
static void init_stats(void *data) {
    *(void **)data = find_interface(1, "STEAMUSERSTATS_INTERFACE_VERSION012");
}
static void init_stats_013(void *data) {
    *(void **)data = find_interface(1, "STEAMUSERSTATS_INTERFACE_VERSION013");
}
static void init_utils(void *data) {
    *(void **)data = find_interface(1, "SteamUtils010");
}

int main(int argc, char **argv) {
    CHECK(argc == 2);
    CHECK(getenv("GAME_MARKER") && strcmp(getenv("GAME_MARKER"), "preserved") == 0);
    CHECK(getenv("SteamAppId") && strcmp(getenv("SteamAppId"), "424242") == 0);
    CHECK(getenv("SteamGameId") && strcmp(getenv("SteamGameId"), "424242") == 0);
    CHECK(dlsym(RTLD_DEFAULT, "lapinux_preload_marker") != NULL);
    const char *expected_provider = getenv("EXPECTED_STEAM_PROVIDER");
    if (expected_provider) {
        Dl_info provider = {0};
        void *resolved = dlsym(RTLD_DEFAULT, "SteamAPI_Init");
        CHECK(resolved && dladdr(resolved, &provider) && provider.dli_fname);
        char actual[4096], expected[4096];
        CHECK(realpath(provider.dli_fname, actual) && realpath(expected_provider, expected));
        fprintf(stderr, "SteamAPI_Init provider=%s expected=%s\n", actual, expected);
        CHECK(strcmp(actual, expected) == 0);
        fprintf(stderr, "verified SteamAPI_Init provider=%s\n", actual);
    }
    if (strcmp(argv[1], "no-session") == 0) {
        int (*init_ex)(const char *, char *) = dlsym(RTLD_DEFAULT, "SteamInternal_SteamAPI_Init");
        char error[1024] = {0};
        CHECK(init_ex && init_ex("", error) != 0 && error[0] != 0);
        CHECK(!SteamAPI_Init());
        puts("enabled: missing session rejected");
        return 0;
    }
    if (strcmp(argv[1], "disabled") == 0) {
        Dl_info provider = {0};
        dladdr(dlsym(RTLD_DEFAULT, "SteamAPI_Init"), &provider);
        fprintf(stderr, "provider=%s LD_PRELOAD=%s LD_LIBRARY_PATH=%s\n", provider.dli_fname ? provider.dli_fname : "?", getenv("LD_PRELOAD") ? getenv("LD_PRELOAD") : "", getenv("LD_LIBRARY_PATH") ? getenv("LD_LIBRARY_PATH") : "");
        CHECK(!SteamAPI_Init());
        CHECK(dlsym(RTLD_DEFAULT, "SteamAPI_SteamUserStats_v012") == NULL);
        puts("disabled: original library and existing preload");
        return 0;
    }
    CHECK(strcmp(argv[1], "enabled") == 0 || strcmp(argv[1], "spool-failure") == 0);
    CHECK(SteamAPI_Init());
    int (*init_ex)(const char *, char *) = dlsym(RTLD_DEFAULT, "SteamInternal_SteamAPI_Init");
    int (*init_flat)(char *) = dlsym(RTLD_DEFAULT, "SteamAPI_InitFlat");
    void *(*context_init)(void *) = dlsym(RTLD_DEFAULT, "SteamInternal_ContextInit");
    find_interface = dlsym(RTLD_DEFAULT, "SteamInternal_FindOrCreateUserInterface");
    CHECK(init_ex && init_flat && context_init && find_interface);
    char error[1024] = {1};
    CHECK(init_ex("", error) == 0 && error[0] == 0);
    CHECK(init_flat(error) == 0);

    struct context stats = { init_stats, 0, NULL };
    CHECK(context_init(&stats) == &stats.object && stats.object);
    uintptr_t *vtable = *(uintptr_t **)stats.object;
    bool (*set_int)(void *, const char *, int32_t) = (void *)vtable[3];
    bool (*get_int)(void *, const char *, int32_t *) = (void *)vtable[1];
    bool (*set_achievement)(void *, const char *) = (void *)vtable[7];
    bool (*get_achievement)(void *, const char *, bool *) = (void *)vtable[6];
    bool (*store_stats)(void *) = (void *)vtable[10];
    int32_t score = 0;
    bool achieved = false;
    CHECK(set_int(stats.object, "score", 42));
    CHECK(get_int(stats.object, "score", &score) && score == 42);
    CHECK(set_achievement(stats.object, "ACH_TEST"));
    CHECK(get_achievement(stats.object, "ACH_TEST", &achieved) && achieved);
    if (strcmp(argv[1], "spool-failure") == 0) {
        CHECK(!store_stats(stats.object));
        puts("enabled: spool failure reported");
        return 0;
    }
    CHECK(store_stats(stats.object));
    /* Current 013 has no RequestCurrentStats at slot zero. */
    struct context stats013 = { init_stats_013, 0, NULL };
    CHECK(context_init(&stats013) == &stats013.object && stats013.object);
    uintptr_t *v013 = *(uintptr_t **)stats013.object;
    bool (*get_int_013)(void *, const char *, int32_t *) = (void *)v013[0];
    CHECK(get_int_013(stats013.object, "score", &score) && score == 42);
    bool (*flat_get)(void *, const char *, int32_t *) = dlsym(RTLD_DEFAULT, "SteamAPI_ISteamUserStats_GetStatInt32");
    CHECK(flat_get && flat_get(stats.object, "score", &score) && score == 42);

    struct context utils = { init_utils, 0, NULL };
    CHECK(context_init(&utils) == &utils.object && utils.object);
    uintptr_t *utils_table = *(uintptr_t **)utils.object;
    uint32_t (*app_id)(void *) = (void *)utils_table[9];
    CHECK(app_id(utils.object) == 424242);
    void *user = find_interface(1, "SteamUser023");
    CHECK(user);
    uintptr_t *user_table = *(uintptr_t **)user;
    bool (*logged_on)(void *) = (void *)user_table[1];
    CHECK(logged_on(user));
    puts("enabled: native bridge, flat API, C++ contexts, stats and achievement");
    return 0;
}
