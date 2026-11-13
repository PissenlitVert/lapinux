// Client OpenGL de validation pour l'interposeur, en GLX sous X11 (Xwayland).
//
// Les en-têtes X11 et GLX ne sont pas installés sur cette machine : les types,
// les structures et les quelques constantes utiles sont déclarés ici, et les
// bibliothèques sont ouvertes par dlopen. Le rendu est du GL 2.1 immédiat
// (asciiClear/scissor), donc sans nuanceur ni table de sommets.
//
// Ce que ce client apporte à la validation : un battement par seconde sur la
// sortie standard (« seconde N : F images, WxH »), ce qui permet d'attribuer
// les images de chaque phase (×2, ×0, ×3) à partir d'un journal réel plutôt
// que d'une moyenne ; et un redimensionnement demandé de l'intérieur, pendant
// que les images défilent.
//
// Usage : gl-client-x11 [secondes] [seconde_du_redimensionnement]
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

/* --- X11, sans en-têtes --------------------------------------------------- */

typedef struct _XDisplay Display;
typedef unsigned long Window, XID, Pixmap, Colormap, Cursor, VisualID, Atom;
typedef struct _XVisual Visual;

typedef struct {
    Pixmap background_pixmap;
    unsigned long background_pixel;
    Pixmap border_pixmap;
    unsigned long border_pixel;
    int bit_gravity;
    int win_gravity;
    int backing_store;
    unsigned long backing_planes;
    unsigned long backing_pixel;
    int save_under;
    long event_mask;
    long do_not_propagate_mask;
    int override_redirect;
    Colormap colormap;
    Cursor cursor;
} XSetWindowAttributes;

typedef struct {
    Visual *visual;
    VisualID visualid;
    int screen;
    int depth;
    int class_;
    unsigned long red_mask, green_mask, blue_mask;
    int colormap_size;
    int bits_per_rgb;
} XVisualInfo;

/* XEvent fait 24 longs ; seul ConfigureNotify nous intéresse (taille). */
typedef struct {
    int type;
    int pad0;
    unsigned long serial;
    int send_event;
    int pad1;
    Display *display;
    Window event;
    Window window;
    int x, y;
    int width, height;
    int border_width;
    int pad2;
    Window above;
    int override_redirect;
    int pad3;
} XConfigureEvent;

typedef union {
    XConfigureEvent configure;
    long pad[24];
} XEvent;

#define InputOutput 1
#define CWColormap (1L << 13)
#define CWEventMask (1L << 11)
#define StructureNotifyMask (1L << 17)
#define ConfigureNotify 22
#define DestroyNotify 17
#define AllocNone 0

static Display *(*p_XOpenDisplay)(const char *);
static int (*p_XDefaultScreen)(Display *);
static Window (*p_XRootWindow)(Display *, int);
static Colormap (*p_XCreateColormap)(Display *, Window, Visual *, int);
static Window (*p_XCreateWindow)(Display *, Window, int, int, unsigned, unsigned, unsigned, int,
                                unsigned, Visual *, unsigned long, XSetWindowAttributes *);
static int (*p_XStoreName)(Display *, Window, const char *);
static int (*p_XMapWindow)(Display *, Window);
static int (*p_XMapRaised)(Display *, Window);
static int (*p_XRaiseWindow)(Display *, Window);
static int (*p_XFlush)(Display *);
static int (*p_XSync)(Display *, int);
static int (*p_XPending)(Display *);
static int (*p_XNextEvent)(Display *, XEvent *);
static int (*p_XResizeWindow)(Display *, Window, unsigned, unsigned);
static int (*p_XDestroyWindow)(Display *, Window);
static int (*p_XCloseDisplay)(Display *);

/* --- GLX et GL ------------------------------------------------------------ */

typedef struct __GLXFBConfigRec *GLXFBConfig;
typedef struct __GLXcontextRec *GLXContext;

#define GLX_RGBA 4
#define GLX_DOUBLEBUFFER 5
#define GLX_RED_SIZE 8
#define GLX_GREEN_SIZE 9
#define GLX_BLUE_SIZE 10
#define GL_COLOR_BUFFER_BIT 0x00004000
#define GL_SCISSOR_TEST 0x0C11
#define GL_RENDERER 0x1F01

/* Le chemin historique (`glXChooseVisual` + `glXCreateContext`) est celui
 * qu'emprunte glxgears, seul client GLX dont l'échange ne se bloque pas sur
 * cette pile Xwayland + NVIDIA : on s'aligne dessus. */
static XVisualInfo *(*p_glXChooseVisual)(Display *, int, int *);
static GLXContext (*p_glXCreateContext)(Display *, XVisualInfo *, GLXContext, int);
static int (*p_glXMakeCurrent)(Display *, unsigned long, GLXContext);
static void (*p_glXSwapBuffers)(Display *, unsigned long);
static void *(*p_glXGetProcAddressARB)(const unsigned char *);

static void (*gl_clear)(uint32_t);
static void (*gl_clear_color)(float, float, float, float);
static void (*gl_enable)(uint32_t);
static void (*gl_scissor)(int, int, int, int);
static void (*gl_viewport)(int, int, int, int);
static void (*gl_finish)(void);
static const unsigned char *(*gl_get_string)(uint32_t);

static void load(void) {
    void *x11 = dlopen("libX11.so.6", RTLD_NOW | RTLD_GLOBAL);
    /* RTLD_GLOBAL puis dlsym(RTLD_DEFAULT) : la portée globale fait passer
     * LD_PRELOAD avant libGL, sans quoi l'interposeur ne verrait jamais
     * glXSwapBuffers (le client l'appellerait directement). */
    void *gl = dlopen("libGL.so.1", RTLD_NOW | RTLD_GLOBAL);
    if (x11 == NULL || gl == NULL) {
        fprintf(stderr, "libX11/libGL absentes\n");
        exit(1);
    }
#define X(fn) *(void **)&p_##fn = dlsym(x11, #fn)
    X(XOpenDisplay);
    X(XDefaultScreen);
    X(XRootWindow);
    X(XCreateColormap);
    X(XCreateWindow);
    X(XStoreName);
    X(XMapWindow);
    X(XMapRaised);
    X(XRaiseWindow);
    X(XFlush);
    X(XSync);
    X(XPending);
    X(XNextEvent);
    X(XResizeWindow);
    X(XDestroyWindow);
    X(XCloseDisplay);
#undef X
#define G(fn) *(void **)&p_##fn = dlsym(RTLD_DEFAULT, #fn)
    G(glXChooseVisual);
    G(glXCreateContext);
    G(glXMakeCurrent);
    G(glXSwapBuffers);
    G(glXGetProcAddressARB);
#undef G
    *(void **)&gl_clear = p_glXGetProcAddressARB((const unsigned char *)"glClear");
    *(void **)&gl_clear_color = p_glXGetProcAddressARB((const unsigned char *)"glClearColor");
    *(void **)&gl_enable = p_glXGetProcAddressARB((const unsigned char *)"glEnable");
    *(void **)&gl_scissor = p_glXGetProcAddressARB((const unsigned char *)"glScissor");
    *(void **)&gl_viewport = p_glXGetProcAddressARB((const unsigned char *)"glViewport");
    *(void **)&gl_finish = p_glXGetProcAddressARB((const unsigned char *)"glFinish");
    *(void **)&gl_get_string = p_glXGetProcAddressARB((const unsigned char *)"glGetString");
}

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

int main(int argc, char **argv) {
    int seconds = argc > 1 ? atoi(argv[1]) : 20;
    int resize_at = argc > 2 ? atoi(argv[2]) : seconds * 3 / 5;
    load();

    Display *dpy = p_XOpenDisplay(NULL);
    if (dpy == NULL) {
        fprintf(stderr, "connexion X11 impossible\n");
        return 1;
    }
    int screen = p_XDefaultScreen(dpy);
    Window root = p_XRootWindow(dpy, screen);
    int attributes[] = {GLX_RGBA,      GLX_DOUBLEBUFFER, GLX_RED_SIZE,   8,
                        GLX_GREEN_SIZE, 8,               GLX_BLUE_SIZE,  8,
                        0};
    XVisualInfo *visual = p_glXChooseVisual(dpy, screen, attributes);
    if (visual == NULL) {
        fprintf(stderr, "aucun visual GLX\n");
        return 1;
    }
    Colormap colormap = p_XCreateColormap(dpy, root, visual->visual, AllocNone);
    XSetWindowAttributes attrs;
    memset(&attrs, 0, sizeof attrs);
    attrs.colormap = colormap;
    attrs.event_mask = StructureNotifyMask;
    int width = 900, height = 520;
    Window window = p_XCreateWindow(dpy, root, 0, 0, (unsigned)width, (unsigned)height, 0,
                                    visual->depth, InputOutput, visual->visual,
                                    CWColormap | CWEventMask, &attrs);
    if (window == 0) {
        fprintf(stderr, "fenêtre X11 impossible\n");
        return 1;
    }
    p_XStoreName(dpy, window, "lapinux-gl-client-x11");
    /* Au premier plan : un compositeur qui cache complètement une fenêtre ne
     * consomme plus ses tampons, et le client se bloque au deuxième échange. */
    if (p_XMapRaised != NULL)
        p_XMapRaised(dpy, window);
    else
        p_XMapWindow(dpy, window);
    p_XSync(dpy, 0);

    GLXContext context = p_glXCreateContext(dpy, visual, NULL, 1);
    if (context == NULL || !p_glXMakeCurrent(dpy, window, context)) {
        fprintf(stderr, "contexte GLX impossible\n");
        return 1;
    }
    printf("client X11 : %s, %dx%d\n", gl_get_string(GL_RENDERER), width, height);
    fflush(stdout);

    gl_enable(GL_SCISSOR_TEST);
    double start = now_ms();
    long frames = 0, resizes = 0, last_beat = 0;
    int resized = 0;
    while (now_ms() - start < (double)seconds * 1e3) {
        double t = now_ms() - start;
        /* Les événements de la fenêtre : un vrai redimensionnement par le
         * gestionnaire de fenêtres arrive aussi par ici. */
        while (p_XPending(dpy) > 0) {
            XEvent event;
            p_XNextEvent(dpy, &event);
            if (event.configure.type == ConfigureNotify) {
                width = event.configure.width;
                height = event.configure.height;
            }
        }
        /* resize_at <= 0 : pas de redimensionnement dans cette passe. */
        if (!resized && resize_at > 0 && t > (double)resize_at * 1e3) {
            resized = 1;
            width = 1100;
            height = 620;
            p_XResizeWindow(dpy, window, (unsigned)width, (unsigned)height);
            p_XFlush(dpy);
            resizes++;
        }
        gl_viewport(0, 0, width, height);
        gl_clear_color(0.04f, 0.05f, 0.07f, 1.0f);
        gl_clear(GL_COLOR_BUFFER_BIT);
        float phase = (float)(t / 1000.0);
        int x = (int)((0.5f + 0.5f * (float)sin(phase * 3.0)) * (float)(width - 160));
        gl_scissor(x, 0, 160, height);
        gl_clear_color(0.36f, 0.72f, 0.45f, 1.0f);
        gl_clear(GL_COLOR_BUFFER_BIT);
        gl_scissor(x, height / 2 - 40, 160, 80);
        gl_clear_color(0.90f, 0.55f, 0.20f, 1.0f);
        gl_clear(GL_COLOR_BUFFER_BIT);
        gl_finish();
        p_glXSwapBuffers(dpy, window);
        p_XFlush(dpy);
        frames++;
        long beat = (long)((now_ms() - start) / 1000.0);
        if (beat > last_beat) {
            last_beat = beat;
            printf("seconde %ld : %ld images, %dx%d\n", beat, frames, width, height);
            fflush(stdout);
        }
        struct timespec pause = {.tv_sec = 0, .tv_nsec = 4000000};
        nanosleep(&pause, NULL);
    }
    printf("client X11 : %ld images, %.1f fps, %ld redimensionnement(s)\n", frames,
           (double)frames / ((now_ms() - start) / 1e3), resizes);
    fflush(stdout);

    /* Ordre de démontage : libGLX_nvidia appelle xcb_present_select_input en
     * démontant et libxcb écrit alors hors bornes (« buffer overflow detected »,
     * SIGABRT) si la swapchain Vulkan de l'interposeur vit encore — défaut
     * externe, sensible à l'ordre seulement. On ferme donc Vulkan ici, tant que
     * la fenêtre et le contexte GLX existent, avant le démontage standard.
     * LAPINUX_GL_CLIENT_TEARDOWN=raw saute cette fermeture pour reproduire le
     * défaut et prouver qu'il est bien externe. */
    const char *teardown = getenv("LAPINUX_GL_CLIENT_TEARDOWN");
    if (teardown == NULL || strcmp(teardown, "raw") != 0) {
        void (*finish)(void) = NULL;
        *(void **)&finish = dlsym(RTLD_DEFAULT, "lapinux_gl_afmf_finish");
        if (finish != NULL)
            finish();
        fflush(NULL);
    }
    p_glXMakeCurrent(dpy, 0, NULL);
    p_XDestroyWindow(dpy, window);
    p_XCloseDisplay(dpy);
    return 0;
}
