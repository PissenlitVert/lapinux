// Client OpenGL de validation pour l'interposeur, en Wayland natif.
//
// La machine n'a ni en-têtes Wayland ni `wayland-scanner` : les interfaces
// xdg-shell utilisées ici (surface, toplevel, configure) sont donc écrites à
// la main, avec les mêmes signatures que le protocole. Le rendu est du GLES2
// pur (aucun nuanceur) : un dégradé qui tourne, dessiné par `glClear` sous
// test de scissor, ce qui suffit à prouver que les images arrivent, changent
// et survivent aux changements de taille.
//
// Le client : crée une fenêtre 900×520, présente en continu à ~144 Hz, change
// sa taille de tampon en cours de route (900×520 → 1100×620) et sort
// proprement. Il imprime son bilan (images, fps, redimensionnements).
//
// Compilation : scripts/build-gl-interpose.sh

#define _GNU_SOURCE
#include <dlfcn.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* --- Wayland, sans en-têtes ---------------------------------------------- */

struct wl_interface;
struct wl_message {
    const char *name;
    const char *signature;
    const struct wl_interface *const *types;
};
struct wl_interface {
    const char *name;
    int version;
    int method_count;
    const struct wl_message *methods;
    int event_count;
    const struct wl_message *events;
};
struct wl_list {
    struct wl_list *prev;
    struct wl_list *next;
};
struct wl_listener {
    struct wl_list link;
    void (*notify)(struct wl_listener *, void *);
};
struct wl_proxy;
struct wl_display;
struct wl_registry;
struct wl_compositor;
struct wl_surface;
struct wl_egl_window;
struct xdg_wm_base;
struct xdg_surface;
struct xdg_toplevel;

static struct wl_interface wl_surface_interface;
static struct wl_interface wl_compositor_interface;
static struct wl_interface wl_registry_interface;
static struct wl_interface xdg_wm_base_interface;
static struct wl_interface wl_region_interface;
static struct wl_interface wl_callback_interface;
static struct wl_interface wl_buffer_interface;
static struct wl_interface xdg_surface_interface;
static struct wl_interface xdg_toplevel_interface;
static struct wl_interface xdg_positioner_interface;
static struct wl_interface xdg_popup_interface;

static const struct wl_interface *surface_types[] = {&wl_surface_interface};
static const struct wl_interface *region_types[] = {&wl_region_interface};
static const struct wl_interface *callback_types[] = {&wl_callback_interface};
static const struct wl_interface *buffer_types[] = {&wl_buffer_interface};
static const struct wl_interface *xdg_surface_types[] = {&xdg_surface_interface};
static const struct wl_interface *xdg_toplevel_types[] = {&xdg_toplevel_interface};
static const struct wl_interface *xdg_positioner_types[] = {&xdg_positioner_interface};

static const struct wl_message compositor_methods[] = {
    {"create_surface", "n", surface_types},
    {"create_region", "n", region_types},
};
static const struct wl_message surface_methods[] = {
    {"destroy", "", NULL},
    {"attach", "?oii", buffer_types},
    {"damage", "iiii", NULL},
    {"frame", "n", callback_types},
    {"set_opaque_region", "?o", region_types},
    {"set_input_region", "?o", region_types},
    {"commit", "", NULL},
    {"set_buffer_transform", "i", NULL},
    {"set_buffer_scale", "i", NULL},
    {"damage_buffer", "iiii", NULL},
    {"offset", "ii", NULL},
};
static const struct wl_message surface_events[] = {
    {"enter", "o", NULL},
    {"leave", "o", NULL},
    {"preferred_buffer_scale", "i", NULL},
    {"preferred_buffer_transform", "u", NULL},
};
static const struct wl_message wm_base_methods[] = {
    {"destroy", "", NULL},
    {"create_positioner", "n", xdg_positioner_types},
    {"get_xdg_surface", "no", xdg_surface_types},
    {"pong", "u", NULL},
};
static const struct wl_message wm_base_events[] = {
    {"ping", "u", NULL},
};
static const struct wl_message xdg_surface_methods[] = {
    {"destroy", "", NULL},
    {"get_toplevel", "n", xdg_toplevel_types},
    {"get_popup", "noo", xdg_positioner_types},
    {"set_window_geometry", "iiii", NULL},
    {"ack_configure", "u", NULL},
};
static const struct wl_message xdg_surface_events[] = {
    {"configure", "u", NULL},
};
static const struct wl_message xdg_toplevel_methods[] = {
    {"destroy", "", NULL},
    {"set_parent", "?o", NULL},
    {"set_title", "s", NULL},
    {"set_app_id", "s", NULL},
    {"show_window_menu", "ouii", NULL},
    {"move", "?ou", NULL},
    {"resize", "?oui", NULL},
    {"set_max_size", "ii", NULL},
    {"set_min_size", "ii", NULL},
    {"set_maximized", "", NULL},
    {"unset_maximized", "", NULL},
    {"set_fullscreen", "?o", NULL},
    {"unset_fullscreen", "", NULL},
    {"set_minimized", "", NULL},
};
static const struct wl_message xdg_toplevel_events[] = {
    {"configure", "iia", NULL},
    {"close", "", NULL},
    {"configure_bounds", "ii", NULL},
    {"wm_capabilities", "a", NULL},
};

static void *wl_client_handle;

static void init_interfaces(void) {
    /* Les interfaces de base viennent de la bibliothèque (leurs tables de
     * messages sont celles du protocole) ; seules celles d'xdg-shell, qui
     * vit dans l'application, sont écrites ici. */
    if (wl_client_handle != NULL) {
        const char *names[] = {"wl_surface_interface", "wl_region_interface",
                               "wl_callback_interface", "wl_buffer_interface",
                               "wl_compositor_interface", "wl_registry_interface"};
        struct wl_interface *slots[] = {&wl_surface_interface, &wl_region_interface,
                                        &wl_callback_interface, &wl_buffer_interface,
                                        &wl_compositor_interface, &wl_registry_interface};
        for (int i = 0; i < 6; i++) {
            struct wl_interface *found = dlsym(wl_client_handle, names[i]);
            if (found != NULL)
                *slots[i] = *found;
        }
    }
    if (wl_compositor_interface.name == NULL)
        wl_compositor_interface = (struct wl_interface){"wl_compositor", 4, 2, compositor_methods, 0, NULL};
    xdg_wm_base_interface = (struct wl_interface){"xdg_wm_base", 6, 4, wm_base_methods, 1, wm_base_events};
    wl_surface_interface = (struct wl_interface){
        "wl_surface", 6, 11, surface_methods, 4, surface_events};
    wl_region_interface = (struct wl_interface){"wl_region", 1, 0, NULL, 0, NULL};
    wl_callback_interface = (struct wl_interface){"wl_callback", 1, 0, NULL, 0, NULL};
    wl_buffer_interface = (struct wl_interface){"wl_buffer", 1, 0, NULL, 0, NULL};
    xdg_surface_interface =
        (struct wl_interface){"xdg_surface", 6, 5, xdg_surface_methods, 1, xdg_surface_events};
    xdg_toplevel_interface =
        (struct wl_interface){"xdg_toplevel", 6, 14, xdg_toplevel_methods, 4, xdg_toplevel_events};
    xdg_positioner_interface =
        (struct wl_interface){"xdg_positioner", 6, 0, NULL, 0, NULL};
    xdg_popup_interface = (struct wl_interface){"xdg_popup", 6, 0, NULL, 0, NULL};
}

/* --- fonctions Wayland et EGL -------------------------------------------- */

static struct wl_display *(*p_wl_display_connect)(const char *);
static void (*p_wl_display_disconnect)(struct wl_display *);
static int (*p_wl_display_roundtrip)(struct wl_display *);
static int (*p_wl_display_dispatch_pending)(struct wl_display *);
static int (*p_wl_display_flush)(struct wl_display *);
static void *(*p_wl_proxy_marshal_flags)(struct wl_proxy *, uint32_t,
                                        const struct wl_interface *, uint32_t, uint32_t, ...);
static int (*p_wl_proxy_add_listener)(struct wl_proxy *, void (**)(void), void *);
static struct wl_egl_window *(*p_wl_egl_window_create)(struct wl_surface *, int, int);
static void (*p_wl_egl_window_resize)(struct wl_egl_window *, int, int, int, int);
static void (*p_wl_egl_window_destroy)(struct wl_egl_window *);

static void load_wayland(void) {
    void *client = dlopen("libwayland-client.so.0", RTLD_NOW);
    wl_client_handle = client;
    void *egl = dlopen("libwayland-egl.so.1", RTLD_NOW);
    if (client == NULL || egl == NULL) {
        fprintf(stderr, "wayland-client/libwayland-egl absents\n");
        exit(1);
    }
    *(void **)&p_wl_display_connect = dlsym(client, "wl_display_connect");
    *(void **)&p_wl_display_disconnect = dlsym(client, "wl_display_disconnect");
    *(void **)&p_wl_display_roundtrip = dlsym(client, "wl_display_roundtrip");
    *(void **)&p_wl_display_dispatch_pending = dlsym(client, "wl_display_dispatch_pending");
    *(void **)&p_wl_display_flush = dlsym(client, "wl_display_flush");
    *(void **)&p_wl_proxy_marshal_flags = dlsym(client, "wl_proxy_marshal_flags");
    *(void **)&p_wl_proxy_add_listener = dlsym(client, "wl_proxy_add_listener");
    *(void **)&p_wl_egl_window_create = dlsym(egl, "wl_egl_window_create");
    *(void **)&p_wl_egl_window_resize = dlsym(egl, "wl_egl_window_resize");
    *(void **)&p_wl_egl_window_destroy = dlsym(egl, "wl_egl_window_destroy");
}

typedef int32_t EGLint;
typedef uint32_t EGLBoolean, EGLenum;
typedef void *EGLDisplay, *EGLConfig, *EGLSurface, *EGLContext;

#define EGL_PLATFORM_WAYLAND_KHR 0x31D8
#define EGL_SURFACE_TYPE 0x3033
#define EGL_WINDOW_BIT 0x0004
#define EGL_RENDERABLE_TYPE 0x3040
#define EGL_OPENGL_ES2_BIT 0x0004
#define EGL_NONE 0x3038
#define EGL_CONTEXT_CLIENT_VERSION 0x3098
#define GL_COLOR_BUFFER_BIT 0x00004000
#define GL_SCISSOR_TEST 0x0C11
#define GL_RENDERER 0x1F01

static EGLDisplay (*p_eglGetPlatformDisplay)(EGLenum, void *, const EGLint *);
static EGLBoolean (*p_eglInitialize)(EGLDisplay, EGLint *, EGLint *);
static EGLBoolean (*p_eglChooseConfig)(EGLDisplay, const EGLint *, EGLConfig *, EGLint, EGLint *);
static EGLSurface (*p_eglCreateWindowSurface)(EGLDisplay, EGLConfig, void *, const EGLint *);
static EGLContext (*p_eglCreateContext)(EGLDisplay, EGLConfig, EGLContext, const EGLint *);
static EGLBoolean (*p_eglMakeCurrent)(EGLDisplay, EGLSurface, EGLSurface, EGLContext);
static EGLBoolean (*p_eglSwapBuffers)(EGLDisplay, EGLSurface);
static void *(*p_eglGetProcAddress)(const char *);

static void load_egl(void) {
    /* RTLD_GLOBAL puis dlsym(RTLD_DEFAULT) : la recherche part de la portée
     * globale, où LD_PRELOAD passe avant la bibliothèque — c'est ce qui permet
     * à l'interposeur d'attraper eglSwapBuffers d'un client qui n'est pas lié
     * à libEGL à l'édition de liens. */
    void *egl = dlopen("libEGL.so.1", RTLD_NOW | RTLD_GLOBAL);
    if (egl == NULL) {
        fprintf(stderr, "libEGL absente\n");
        exit(1);
    }
    *(void **)&p_eglGetPlatformDisplay = dlsym(RTLD_DEFAULT, "eglGetPlatformDisplay");
    *(void **)&p_eglInitialize = dlsym(RTLD_DEFAULT, "eglInitialize");
    *(void **)&p_eglChooseConfig = dlsym(RTLD_DEFAULT, "eglChooseConfig");
    *(void **)&p_eglCreateWindowSurface = dlsym(RTLD_DEFAULT, "eglCreateWindowSurface");
    *(void **)&p_eglCreateContext = dlsym(RTLD_DEFAULT, "eglCreateContext");
    *(void **)&p_eglMakeCurrent = dlsym(RTLD_DEFAULT, "eglMakeCurrent");
    *(void **)&p_eglSwapBuffers = dlsym(RTLD_DEFAULT, "eglSwapBuffers");
    *(void **)&p_eglGetProcAddress = dlsym(RTLD_DEFAULT, "eglGetProcAddress");
}

static void (*gl_clear)(uint32_t);
static void (*gl_clear_color)(float, float, float, float);
static void (*gl_enable)(uint32_t);
static void (*gl_scissor)(int, int, int, int);
static void (*gl_finish)(void);
static const unsigned char *(*gl_get_string)(uint32_t);

static void load_gl(void) {
    *(void **)&gl_clear = p_eglGetProcAddress("glClear");
    *(void **)&gl_clear_color = p_eglGetProcAddress("glClearColor");
    *(void **)&gl_enable = p_eglGetProcAddress("glEnable");
    *(void **)&gl_scissor = p_eglGetProcAddress("glScissor");
    *(void **)&gl_finish = p_eglGetProcAddress("glFinish");
    *(void **)&gl_get_string = p_eglGetProcAddress("glGetString");
}

/* --- fenêtre xdg-shell ---------------------------------------------------- */

struct state {
    struct wl_display *display;
    struct wl_registry *registry;
    struct wl_compositor *compositor;
    struct xdg_wm_base *wm_base;
    struct wl_surface *surface;
    struct xdg_surface *xdg_surface;
    struct xdg_toplevel *toplevel;
    struct wl_egl_window *egl_window;
    int width, height;
    int configured;
    int closed;
};

static struct state s = {.width = 900, .height = 520};

static void registry_global(void *data, struct wl_registry *registry, uint32_t name,
                            const char *interface, uint32_t version) {
    struct state *st = data;
    if (strcmp(interface, "wl_compositor") == 0) {
        uint32_t v = version < 4 ? version : 4;
        st->compositor = p_wl_proxy_marshal_flags((struct wl_proxy *)registry, 0,
                                                  &wl_compositor_interface, v, 0, name,
                                                  "wl_compositor", v, (void *)NULL);
    } else if (strcmp(interface, "xdg_wm_base") == 0) {
        uint32_t v = version < 6 ? version : 6;
        st->wm_base = p_wl_proxy_marshal_flags((struct wl_proxy *)registry, 0,
                                               &xdg_wm_base_interface, v, 0, name,
                                               "xdg_wm_base", v, (void *)NULL);
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static const struct {
    void (*global)(void *, struct wl_registry *, uint32_t, const char *, uint32_t);
    void (*global_remove)(void *, struct wl_registry *, uint32_t);
} registry_listener = {registry_global, registry_global_remove};

static void wm_base_ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
    (void)data;
    p_wl_proxy_marshal_flags((struct wl_proxy *)base, 3, NULL, 6, 0, serial);
}

static const struct {
    void (*ping)(void *, struct xdg_wm_base *, uint32_t);
} wm_base_listener = {wm_base_ping};

static void xdg_surface_configure(void *data, struct xdg_surface *surface, uint32_t serial) {
    struct state *st = data;
    p_wl_proxy_marshal_flags((struct wl_proxy *)surface, 4, NULL, 6, 0, serial);
    st->configured = 1;
}

static const struct {
    void (*configure)(void *, struct xdg_surface *, uint32_t);
} xdg_surface_listener = {xdg_surface_configure};

static void toplevel_configure(void *data, struct xdg_toplevel *toplevel, int32_t width,
                               int32_t height, void *states) {
    (void)toplevel;
    (void)states;
    struct state *st = data;
    /* La taille vient du compositeur ; 0 veut dire « choisis toi-même ». */
    if (width > 0 && height > 0) {
        st->width = width;
        st->height = height;
    }
}

static void toplevel_close(void *data, struct xdg_toplevel *toplevel) {
    (void)toplevel;
    ((struct state *)data)->closed = 1;
}

/* Le tableau de rappels suit les opcodes des événements du protocole : il doit
 * avoir une entrée par événement, même sans traitement, sinon libwayland
 * appelle un pointeur hors du tableau. */
static void toplevel_configure_bounds(void *data, struct xdg_toplevel *toplevel, int32_t width,
                                      int32_t height) {
    (void)data;
    (void)toplevel;
    (void)width;
    (void)height;
}

static void toplevel_capabilities(void *data, struct xdg_toplevel *toplevel, void *capabilities) {
    (void)data;
    (void)toplevel;
    (void)capabilities;
}

static const struct {
    void (*configure)(void *, struct xdg_toplevel *, int32_t, int32_t, void *);
    void (*close)(void *, struct xdg_toplevel *);
    void (*configure_bounds)(void *, struct xdg_toplevel *, int32_t, int32_t);
    void (*wm_capabilities)(void *, struct xdg_toplevel *, void *);
} toplevel_listener = {toplevel_configure, toplevel_close, toplevel_configure_bounds,
                       toplevel_capabilities};

static int create_window(struct state *st) {
    st->display = p_wl_display_connect(NULL);
    if (st->display == NULL) {
        fprintf(stderr, "connexion Wayland impossible (%s)\n", getenv("WAYLAND_DISPLAY"));
        return 0;
    }
    /* `wl_display_get_registry` est une fonction inline dans wayland-client :
     * on refait son appel, opcode 1 sur wl_display. */
    st->registry = p_wl_proxy_marshal_flags((struct wl_proxy *)st->display, 1,
                                           &wl_registry_interface, 1, 0, (void *)NULL);
    p_wl_proxy_add_listener((struct wl_proxy *)st->registry, (void (**)(void))&registry_listener,
                            st);
    p_wl_display_roundtrip(st->display);
    if (st->compositor == NULL || st->wm_base == NULL) {
        fprintf(stderr, "wl_compositor ou xdg_wm_base absent\n");
        return 0;
    }
    p_wl_proxy_add_listener((struct wl_proxy *)st->wm_base, (void (**)(void))&wm_base_listener,
                            st);
    st->surface = p_wl_proxy_marshal_flags((struct wl_proxy *)st->compositor, 0,
                                           &wl_surface_interface, 4, 0, (void *)NULL);
    st->xdg_surface = p_wl_proxy_marshal_flags((struct wl_proxy *)st->wm_base, 2,
                                               &xdg_surface_interface, 6, 0, (void *)NULL,
                                               st->surface);
    p_wl_proxy_add_listener((struct wl_proxy *)st->xdg_surface,
                            (void (**)(void))&xdg_surface_listener, st);
    st->toplevel = p_wl_proxy_marshal_flags((struct wl_proxy *)st->xdg_surface, 1,
                                            &xdg_toplevel_interface, 6, 0, (void *)NULL);
    p_wl_proxy_add_listener((struct wl_proxy *)st->toplevel, (void (**)(void))&toplevel_listener,
                            st);
    p_wl_proxy_marshal_flags((struct wl_proxy *)st->toplevel, 2, NULL, 6, 0,
                             "Lapinux : client OpenGL de validation");
    p_wl_proxy_marshal_flags((struct wl_proxy *)st->toplevel, 3, NULL, 6, 0, "lapinux-gl-client");
    p_wl_proxy_marshal_flags((struct wl_proxy *)st->surface, 6, NULL, 4, 0);
    for (int i = 0; i < 100 && !st->configured; i++) {
        if (p_wl_display_roundtrip(st->display) < 0)
            return 0;
    }
    return st->configured;
}

/* --- boucle --------------------------------------------------------------- */

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

int main(int argc, char **argv) {
    int seconds = argc > 1 ? atoi(argv[1]) : 8;
    int resize_at = argc > 2 ? atoi(argv[2]) : seconds / 2;
    load_wayland();
    init_interfaces();
    load_egl();
    if (!create_window(&s))
        return 1;

    s.egl_window = p_wl_egl_window_create(s.surface, s.width, s.height);
    EGLDisplay display = p_eglGetPlatformDisplay(EGL_PLATFORM_WAYLAND_KHR, s.display, NULL);
    if (display == NULL || !p_eglInitialize(display, NULL, NULL)) {
        fprintf(stderr, "EGL Wayland indisponible\n");
        return 1;
    }
    EGLint attributes[] = {EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RENDERABLE_TYPE,
                           EGL_OPENGL_ES2_BIT, EGL_NONE};
    EGLConfig config = NULL;
    EGLint count = 0;
    if (!p_eglChooseConfig(display, attributes, &config, 1, &count) || count == 0) {
        fprintf(stderr, "aucune configuration EGL\n");
        return 1;
    }
    EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    EGLContext context = p_eglCreateContext(display, config, NULL, context_attributes);
    EGLSurface surface = p_eglCreateWindowSurface(display, config, s.egl_window, NULL);
    if (context == NULL || surface == NULL ||
        !p_eglMakeCurrent(display, surface, surface, context)) {
        fprintf(stderr, "contexte OpenGL impossible\n");
        return 1;
    }
    load_gl();
    printf("client Wayland : %s, %dx%d\n", gl_get_string(GL_RENDERER), s.width, s.height);
    fflush(stdout);

    gl_enable(GL_SCISSOR_TEST);
    double start = now_ms();
    long frames = 0, resizes = 0, last_beat = 0;
    int target_width = s.width, target_height = s.height;
    int resized = 0;
    while (!s.closed && now_ms() - start < (double)seconds * 1e3) {
        double t = now_ms() - start;
        /* À mi-parcours, la taille du tampon change : c'est le test du chemin
         * de redimensionnement de l'interposeur. */
        /* resize_at <= 0 : pas de redimensionnement dans cette passe. */
        if (!resized && resize_at > 0 && t > (double)resize_at * 1e3) {
            resized = 1;
            target_width = 1100;
            target_height = 620;
            p_wl_egl_window_resize(s.egl_window, target_width, target_height, 0, 0);
            s.width = target_width;
            s.height = target_height;
            p_wl_proxy_marshal_flags((struct wl_proxy *)s.surface, 6, NULL, 4, 0);
            resizes++;
        }
        gl_clear_color(0.04f, 0.05f, 0.07f, 1.0f);
        gl_clear(GL_COLOR_BUFFER_BIT);
        /* Une bande qui balaie l'écran : deux couleurs, faciles à reconnaître
         * et impossible à confondre avec une image figée. */
        float phase = (float)(t / 1000.0);
        int x = (int)((0.5f + 0.5f * (float)sin(phase * 3.0)) * (float)(s.width - 160));
        gl_clear_color(0.36f, 0.72f, 0.45f, 1.0f); /* vert : la bande */
        gl_scissor(x, 0, 160, s.height);
        gl_clear(GL_COLOR_BUFFER_BIT);
        gl_clear_color(0.90f, 0.55f, 0.20f, 1.0f); /* orange : le curseur */
        gl_scissor(x, s.height / 2 - 40, 160, 80);
        gl_clear(GL_COLOR_BUFFER_BIT);
        gl_finish();
        p_eglSwapBuffers(display, surface);
        p_wl_display_dispatch_pending(s.display);
        p_wl_display_flush(s.display);
        frames++;
        long beat = (long)((now_ms() - start) / 1000.0);
        if (beat > last_beat) {
            last_beat = beat;
            printf("seconde %ld : %ld images, %dx%d\n", beat, frames, s.width, s.height);
            fflush(stdout);
        }
        /* La boucle vise ~144 Hz : un jeu réel ne tourne pas plus vite, et la
         * couche a besoin d'un rythme réaliste pour placer ses images. */
        struct timespec pause = {.tv_sec = 0, .tv_nsec = 4000000};
        nanosleep(&pause, NULL);
    }
    printf("client Wayland : %ld images, %.1f fps, %ld redimensionnement(s)\n", frames,
           (double)frames / ((now_ms() - start) / 1e3), resizes);
    p_eglMakeCurrent(display, NULL, NULL, NULL);
    p_wl_egl_window_destroy(s.egl_window);
    if (s.toplevel != NULL)
        p_wl_proxy_marshal_flags((struct wl_proxy *)s.toplevel, 0, NULL, 6, 0);
    if (s.xdg_surface != NULL)
        p_wl_proxy_marshal_flags((struct wl_proxy *)s.xdg_surface, 0, NULL, 6, 0);
    if (s.surface != NULL)
        p_wl_proxy_marshal_flags((struct wl_proxy *)s.surface, 0, NULL, 4, 0);
    p_wl_display_flush(s.display);
    p_wl_display_disconnect(s.display);
    return 0;
}
