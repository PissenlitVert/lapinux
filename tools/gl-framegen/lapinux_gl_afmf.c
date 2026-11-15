// Interposeur OpenGL -> Vulkan pour la génération d'images de Lapinux.
//
// Les jeux OpenGL présentent par `eglSwapBuffers` ou `glXSwapBuffers`. Cet
// interposeur prend possession de la présentation : à chaque appel, il copie
// le tampon arrière OpenGL dans une texture dont la mémoire vient de Vulkan
// (exportée par fd, `GL_EXT_memory_object_fd`), puis il présente lui-même une
// swapchain Vulkan sur la fenêtre de l'application. La couche afmf-linux
// s'insère sans modification sur cette swapchain et génère les images du
// multiplicateur, exactement comme pour un jeu Vulkan.
//
// Rien ne traverse la mémoire du processeur : la copie est un
// `glCopyTexSubImage2D` côté GPU et la présentation un `vkCmdBlitImage`.
//
// Repli : si quoi que ce soit manque (pas de fenêtre X11, pas de Vulkan, pas
// d'export de mémoire, swapchain perdue, génération coupée), l'appel est
// rendu à la fonction d'origine et le jeu continue en OpenGL natif. Un seul
// échec désactive l'interposeur pour de bon : mieux vaut une image de moins
// qu'un jeu qui se fige.
//
// Variables d'environnement :
//   LAPINUX_GL_AFMF=0        désactive l'interposeur (repli immédiat)
//   LAPINUX_GL_AFMF_LOG=0    coupe les compteurs sur la sortie d'erreur
//   LAPINUX_GL_AFMF_VERIFY=1 compare une fois, côté Vulkan et côté OpenGL,
//                            les pixels de la même image
//   AFMF_ACQUIRE_TIMEOUT_US  budget d'acquisition des images de la couche ;
//                            posé à 8000 µs par l'interposeur s'il est absent
//
// Compilation : scripts/build-gl-interpose.sh

#define _GNU_SOURCE
#include <vulkan/vulkan.h>

#include <dlfcn.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* --- EGL et GLES, sans en-têtes (absents de cette machine) ---------------- */

typedef int32_t EGLint;
typedef uint32_t EGLBoolean, EGLenum;
typedef void *EGLDisplay, *EGLConfig, *EGLSurface, *EGLContext;

typedef struct _XDisplay Display;
typedef unsigned long Window, GLXDrawable, Pixmap, Colormap, Cursor, VisualID;

/* Après les types X11, que cet en-tête attend. */
#include <vulkan/vulkan_xlib.h>
struct wl_display;
struct wl_surface;
#include <vulkan/vulkan_wayland.h>

typedef uint32_t GLenum, GLuint, GLsizei, GLbitfield;
typedef int32_t GLint, GLsizeiptr;
typedef unsigned char GLubyte;
typedef int GLboolean;

#define EGL_PLATFORM_X11_KHR 0x31D5
#define EGL_PLATFORM_WAYLAND_KHR 0x31D8
#define EGL_WIDTH 0x3057
#define EGL_HEIGHT 0x3056
#define GL_TEXTURE_2D 0x0DE1
#define GL_RGBA8 0x8058
#define GL_READ_FRAMEBUFFER_BINDING 0x8CAA
#define GL_DRAW_FRAMEBUFFER_BINDING 0x8CA6
#define GL_TEXTURE_BINDING_2D 0x8069
#define GL_READ_FRAMEBUFFER 0x8CA8
#define GL_READ_BUFFER 0x0C02
#define GL_BACK 0x0405
#define GL_PACK_ALIGNMENT 0x0D05
#define GL_NO_ERROR 0
#define GL_HANDLE_TYPE_OPAQUE_FD_EXT 0x9586
#define GLX_WIDTH 0x801D
#define GLX_HEIGHT 0x801E

static void (*p_glXSwapBuffers)(Display *, GLXDrawable);
static Display *(*p_glXGetCurrentDisplay)(void);
static GLXDrawable (*p_glXGetCurrentDrawable)(void);
static void (*p_glXQueryDrawable)(Display *, GLXDrawable, int, unsigned *, int *);
static void *(*p_glXGetProcAddressARB)(const GLubyte *);

static EGLBoolean (*p_eglSwapBuffers)(EGLDisplay, EGLSurface);
static EGLDisplay (*p_eglGetDisplay)(void *);
static EGLDisplay (*p_eglGetPlatformDisplay)(EGLenum, void *, const EGLint *);
static EGLSurface (*p_eglCreateWindowSurface)(EGLDisplay, EGLConfig, void *, const EGLint *);
static EGLBoolean (*p_eglQuerySurface)(EGLDisplay, EGLSurface, EGLint, EGLint *);
static EGLSurface (*p_eglGetCurrentSurface)(EGLint);
static void *(*p_eglGetProcAddress)(const char *);
static void *(*p_xOpenDisplay)(const char *);

static void (*gl_copy_tex_sub_image)(GLenum, GLint, GLint, GLint, GLint, GLint, GLsizei, GLsizei);
static void (*gl_bind_texture)(GLenum, GLuint);
static void (*gl_read_buffer)(GLenum);
static void (*gl_bind_framebuffer)(GLenum, GLuint);
static void (*gl_gen_textures)(GLsizei, GLuint *);
static void (*gl_delete_textures)(GLsizei, const GLuint *);
static void (*gl_get_integerv)(GLenum, GLint *);
static GLenum (*gl_get_error)(void);
static void (*gl_finish)(void);
static void (*gl_pixel_storei)(GLenum, GLint);
static void (*gl_read_pixels)(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, void *);
static void (*gl_create_memory_objects)(GLsizei, GLuint *);
static void (*gl_delete_memory_objects)(GLsizei, const GLuint *);
static void (*gl_import_memory_fd)(GLuint, uint64_t, GLenum, int);
static void (*gl_tex_storage_mem2d)(GLenum, GLsizei, GLenum, GLsizei, GLsizei, GLuint, uint64_t);

static int g_log = 1;

static void log_line(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void log_line(const char *fmt, ...);
static void read_log_setting(void) {
    const char *value = getenv("LAPINUX_GL_AFMF_LOG");
    if (value != NULL && strcmp(value, "0") == 0)
        g_log = 0;
}

static void log_line(const char *fmt, ...) {
    static int busy;
    if (!g_log || busy)
        return;
    busy = 1;
    va_list ap;
    va_start(ap, fmt);
    fputs("lapinux-gl-afmf: ", stderr);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    fflush(stderr);
    va_end(ap);
    busy = 0;
}

static double now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e6 + (double)ts.tv_nsec / 1e3;
}

/* --- état ---------------------------------------------------------------- */

struct frame_stats {
    unsigned long presents;
    unsigned long copied;
    unsigned long resizes;
    unsigned long failed;
    double copy_us_total;
    double present_us_total;
};

struct interposer {
    int tried;             /* l'initialisation a été tentée */
    int disabled;          /* repli définitif */
    const char *why;       /* raison du repli, journalisée une fois */

    Display *dpy;          /* fenêtre X11 de l'application */
    Window win;
    struct wl_surface *wl_surface; /* fenêtre Wayland de l'application */
    EGLDisplay egl_display;
    EGLSurface egl_surface;
    int egl_x11;           /* 1 quand la surface EGL est une fenêtre X11 */
    int egl_wayland;       /* 1 quand la surface EGL est une fenêtre Wayland */
    int closed_own_display;

    /* Vulkan */
    VkInstance instance;
    VkDebugUtilsMessengerEXT messenger;
    VkPhysicalDevice physical;
    VkDevice device;
    VkQueue queue;
    uint32_t family;
    VkSurfaceKHR surface;
    VkSwapchainKHR swapchain;
    VkFormat format;
    VkExtent2D extent;
    uint32_t image_count;
    VkImage *images;
    VkImageView *views;
    VkCommandPool pool;
    VkCommandBuffer cmd;
    VkFence fence;
    VkSemaphore sem_image;   /* image acquise */
    VkSemaphore sem_drawn;   /* blit terminé */

    /* Texture partagée : image Vulkan exportée, texture OpenGL importée */
    VkImage shared;
    VkDeviceMemory shared_memory;
    int shared_fd;
    VkDeviceSize shared_size;
    GLuint gl_texture;
    GLuint gl_memory;
    uint32_t width;
    uint32_t height;

    struct frame_stats stats;
};

static struct interposer g = {.shared_fd = -1};

static void *open_lib(const char *name) {
    void *handle = dlopen(name, RTLD_NOW | RTLD_GLOBAL);
    return handle;
}

static void load_symbols(void) {
    read_log_setting();
    static int done;
    if (done)
        return;
    done = 1;
    void *gl = open_lib("libGL.so.1");
    void *egl = open_lib("libEGL.so.1");
    void *x11 = open_lib("libX11.so.6");
    if (gl != NULL) {
        *(void **)&p_glXSwapBuffers = dlsym(gl, "glXSwapBuffers");
        *(void **)&p_glXGetCurrentDisplay = dlsym(gl, "glXGetCurrentDisplay");
        *(void **)&p_glXGetCurrentDrawable = dlsym(gl, "glXGetCurrentDrawable");
        *(void **)&p_glXQueryDrawable = dlsym(gl, "glXQueryDrawable");
        *(void **)&p_glXGetProcAddressARB = dlsym(gl, "glXGetProcAddressARB");
    }
    if (egl != NULL) {
        *(void **)&p_eglSwapBuffers = dlsym(egl, "eglSwapBuffers");
        *(void **)&p_eglGetDisplay = dlsym(egl, "eglGetDisplay");
        *(void **)&p_eglGetPlatformDisplay = dlsym(egl, "eglGetPlatformDisplay");
        *(void **)&p_eglCreateWindowSurface = dlsym(egl, "eglCreateWindowSurface");
        *(void **)&p_eglQuerySurface = dlsym(egl, "eglQuerySurface");
        *(void **)&p_eglGetCurrentSurface = dlsym(egl, "eglGetCurrentSurface");
        *(void **)&p_eglGetProcAddress = dlsym(egl, "eglGetProcAddress");
    }
    if (x11 != NULL)
        *(void **)&p_xOpenDisplay = dlsym(x11, "XOpenDisplay");
}

static void *gl_proc(const char *name) {
    void *p = p_glXGetProcAddressARB != NULL ? p_glXGetProcAddressARB((const GLubyte *)name) : NULL;
    if (p == NULL && p_eglGetProcAddress != NULL)
        p = p_eglGetProcAddress(name);
    return p;
}

static int load_gl(void) {
    if (gl_copy_tex_sub_image != NULL)
        return 1;
    if (p_glXGetProcAddressARB == NULL)
        return 0;
#define GLPROC(field, name) *(void **)&field = gl_proc(name)
    GLPROC(gl_copy_tex_sub_image, "glCopyTexSubImage2D");
    GLPROC(gl_bind_texture, "glBindTexture");
    GLPROC(gl_read_buffer, "glReadBuffer");
    GLPROC(gl_bind_framebuffer, "glBindFramebuffer");
    GLPROC(gl_gen_textures, "glGenTextures");
    GLPROC(gl_delete_textures, "glDeleteTextures");
    GLPROC(gl_get_integerv, "glGetIntegerv");
    GLPROC(gl_get_error, "glGetError");
    GLPROC(gl_finish, "glFinish");
    GLPROC(gl_pixel_storei, "glPixelStorei");
    GLPROC(gl_read_pixels, "glReadPixels");
    GLPROC(gl_create_memory_objects, "glCreateMemoryObjectsEXT");
    GLPROC(gl_delete_memory_objects, "glDeleteMemoryObjectsEXT");
    GLPROC(gl_import_memory_fd, "glImportMemoryFdEXT");
    GLPROC(gl_tex_storage_mem2d, "glTexStorageMem2DEXT");
#undef GLPROC
    return gl_copy_tex_sub_image != NULL && gl_tex_storage_mem2d != NULL &&
           gl_create_memory_objects != NULL && gl_import_memory_fd != NULL;
}

/* --- Vulkan -------------------------------------------------------------- */

static int vk_setup(const char *extensions[], uint32_t count, VkInstance *out) {
    VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
                             .pApplicationName = "lapinux-gl-afmf",
                             .apiVersion = VK_API_VERSION_1_1};
    VkInstanceCreateInfo info = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                                 .pApplicationInfo = &app,
                                 .enabledExtensionCount = count,
                                 .ppEnabledExtensionNames = extensions};
    return vkCreateInstance(&info, NULL, out) == VK_SUCCESS;
}

static int device_has(VkPhysicalDevice dev, const char *name) {
    uint32_t n = 0;
    vkEnumerateDeviceExtensionProperties(dev, NULL, &n, NULL);
    VkExtensionProperties *exts = calloc(n ? n : 1, sizeof *exts);
    if (exts == NULL)
        return 0;
    vkEnumerateDeviceExtensionProperties(dev, NULL, &n, exts);
    int found = 0;
    for (uint32_t i = 0; i < n && !found; i++)
        found = strcmp(exts[i].extensionName, name) == 0;
    free(exts);
    return found;
}

static int instance_has(const char *name) {
    uint32_t n = 0;
    vkEnumerateInstanceExtensionProperties(NULL, &n, NULL);
    VkExtensionProperties *exts = calloc(n ? n : 1, sizeof *exts);
    if (exts == NULL)
        return 0;
    vkEnumerateInstanceExtensionProperties(NULL, &n, exts);
    int found = 0;
    for (uint32_t i = 0; i < n && !found; i++)
        found = strcmp(exts[i].extensionName, name) == 0;
    free(exts);
    return found;
}

static int pick_physical(void) {
    uint32_t count = 0;
    if (vkEnumeratePhysicalDevices(g.instance, &count, NULL) != VK_SUCCESS || count == 0)
        return 0;
    VkPhysicalDevice *devices = calloc(count, sizeof *devices);
    if (devices == NULL)
        return 0;
    vkEnumeratePhysicalDevices(g.instance, &count, devices);
    int chosen = 0;
    for (uint32_t i = 0; i < count && !chosen; i++) {
        VkPhysicalDeviceProperties props;
        vkGetPhysicalDeviceProperties(devices[i], &props);
        /* Le processeur n'a pas d'image à présenter à l'écran. */
        if (props.deviceType == VK_PHYSICAL_DEVICE_TYPE_CPU)
            continue;
        if (!device_has(devices[i], VK_KHR_SWAPCHAIN_EXTENSION_NAME) ||
            !device_has(devices[i], VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME))
            continue;
        uint32_t families = 0;
        vkGetPhysicalDeviceQueueFamilyProperties(devices[i], &families, NULL);
        VkQueueFamilyProperties *fams = calloc(families ? families : 1, sizeof *fams);
        if (fams == NULL)
            continue;
        vkGetPhysicalDeviceQueueFamilyProperties(devices[i], &families, fams);
        for (uint32_t f = 0; f < families && !chosen; f++) {
            VkBool32 present = VK_FALSE;
            if (!(fams[f].queueFlags & VK_QUEUE_GRAPHICS_BIT))
                continue;
            VkResult support =
                vkGetPhysicalDeviceSurfaceSupportKHR(devices[i], f, g.surface, &present);
            if (support != VK_SUCCESS || !present)
                continue;
            g.physical = devices[i];
            g.family = f;
            chosen = 1;
        }
        free(fams);
    }
    free(devices);
    return chosen;
}

static int create_swapchain(uint32_t width, uint32_t height, VkSwapchainKHR old) {
    VkSurfaceCapabilitiesKHR caps;
    if (vkGetPhysicalDeviceSurfaceCapabilitiesKHR(g.physical, g.surface, &caps) != VK_SUCCESS)
        return 0;
    uint32_t format_count = 0;
    vkGetPhysicalDeviceSurfaceFormatsKHR(g.physical, g.surface, &format_count, NULL);
    VkSurfaceFormatKHR *formats = calloc(format_count ? format_count : 1, sizeof *formats);
    if (formats == NULL)
        return 0;
    vkGetPhysicalDeviceSurfaceFormatsKHR(g.physical, g.surface, &format_count, formats);
    VkSurfaceFormatKHR format = formats[0];
    int have_rgba = 0;
    for (uint32_t i = 0; i < format_count; i++) {
        if (formats[i].format == VK_FORMAT_R8G8B8A8_UNORM) {
            format = formats[i];
            have_rgba = 1;
        }
    }
    if (!have_rgba)
        for (uint32_t i = 0; i < format_count; i++)
            if (formats[i].format == VK_FORMAT_B8G8R8A8_UNORM)
                format = formats[i];
    free(formats);
    g.format = format.format;

    uint32_t mode_count = 0;
    vkGetPhysicalDeviceSurfacePresentModesKHR(g.physical, g.surface, &mode_count, NULL);
    VkPresentModeKHR *modes = calloc(mode_count ? mode_count : 1, sizeof *modes);
    if (modes == NULL)
        return 0;
    vkGetPhysicalDeviceSurfacePresentModesKHR(g.physical, g.surface, &mode_count, modes);
    /* MAILBOX quand la surface l'offre : l'application garde son rythme et seule
     * la dernière image est retenue. */
    VkPresentModeKHR mode = VK_PRESENT_MODE_FIFO_KHR;
    for (uint32_t i = 0; i < mode_count; i++)
        if (modes[i] == VK_PRESENT_MODE_MAILBOX_KHR)
            mode = VK_PRESENT_MODE_MAILBOX_KHR;
    free(modes);

    VkExtent2D extent = caps.currentExtent;
    if (extent.width == UINT32_MAX) {
        extent.width = width;
        extent.height = height;
        if (extent.width < caps.minImageExtent.width)
            extent.width = caps.minImageExtent.width;
        if (extent.width > caps.maxImageExtent.width)
            extent.width = caps.maxImageExtent.width;
        if (extent.height < caps.minImageExtent.height)
            extent.height = caps.minImageExtent.height;
        if (extent.height > caps.maxImageExtent.height)
            extent.height = caps.maxImageExtent.height;
    }
    if (extent.width == 0 || extent.height == 0)
        return 0;

    VkImageUsageFlags usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
    int dst_ok = (caps.supportedUsageFlags & VK_IMAGE_USAGE_TRANSFER_DST_BIT) != 0;
    if (dst_ok)
        usage |= VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    /* La couche a besoin d'images libres pour placer ses images générées ;
     * une de plus que le minimum laisse le compositeur en garder une sans
     * affamer la génération. */
    uint32_t images = caps.minImageCount + 3;
    if (images > caps.maxImageCount && caps.maxImageCount > 0)
        images = caps.maxImageCount;
    VkCompositeAlphaFlagBitsKHR alpha = (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR)
        ? VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR
        : VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
    VkSwapchainCreateInfoKHR info = {.sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR,
                                     .surface = g.surface,
                                     .minImageCount = images,
                                     .imageFormat = format.format,
                                     .imageColorSpace = format.colorSpace,
                                     .imageExtent = extent,
                                     .imageArrayLayers = 1,
                                     .imageUsage = usage,
                                     .imageSharingMode = VK_SHARING_MODE_EXCLUSIVE,
                                     .preTransform = caps.currentTransform,
                                     .compositeAlpha = alpha,
                                     .presentMode = mode,
                                     .clipped = VK_TRUE,
                                     .oldSwapchain = old};
    VkSwapchainKHR chain = VK_NULL_HANDLE;
    if (vkCreateSwapchainKHR(g.device, &info, NULL, &chain) != VK_SUCCESS)
        return 0;

    if (g.swapchain != VK_NULL_HANDLE)
        vkDestroySwapchainKHR(g.device, g.swapchain, NULL);
    for (uint32_t i = 0; i < g.image_count; i++) {
        if (g.views[i] != VK_NULL_HANDLE)
            vkDestroyImageView(g.device, g.views[i], NULL);
    }
    free(g.images);
    free(g.views);
    g.images = NULL;
    g.views = NULL;
    g.image_count = 0;
    g.swapchain = chain;
    g.extent = extent;

    uint32_t count = 0;
    vkGetSwapchainImagesKHR(g.device, chain, &count, NULL);
    g.images = calloc(count ? count : 1, sizeof *g.images);
    g.views = calloc(count ? count : 1, sizeof *g.views);
    if (g.images == NULL || g.views == NULL)
        return 0;
    if (vkGetSwapchainImagesKHR(g.device, chain, &count, g.images) != VK_SUCCESS)
        return 0;
    g.image_count = count;
    for (uint32_t i = 0; i < count; i++) {
        VkImageViewCreateInfo view = {.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
                                      .image = g.images[i],
                                      .viewType = VK_IMAGE_VIEW_TYPE_2D,
                                      .format = g.format,
                                      .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
        if (vkCreateImageView(g.device, &view, NULL, &g.views[i]) != VK_SUCCESS)
            return 0;
    }
    if (!dst_ok) {
        /* Sans TRANSFER_DST, la copie vers l'image de swapchain est impossible :
         * on le dit une fois plutôt que de présenter des images vides. */
        g.why = "swapchain sans VK_IMAGE_USAGE_TRANSFER_DST_BIT";
        return 0;
    }
    return 1;
}

/* La texture dont la mémoire vient de Vulkan, importée côté OpenGL. Le fd
 * exporté est dupliqué par l'import : on peut le refermer tout de suite. */
static int create_shared(uint32_t width, uint32_t height) {
    if (g.shared != VK_NULL_HANDLE) {
        if (g.gl_texture != 0 && gl_delete_textures != NULL)
            gl_delete_textures(1, &g.gl_texture);
        if (g.gl_memory != 0 && gl_delete_memory_objects != NULL)
            gl_delete_memory_objects(1, &g.gl_memory);
        g.gl_texture = 0;
        g.gl_memory = 0;
        vkDestroyImage(g.device, g.shared, NULL);
        g.shared = VK_NULL_HANDLE;
        vkFreeMemory(g.device, g.shared_memory, NULL);
        g.shared_memory = VK_NULL_HANDLE;
        if (g.shared_fd >= 0)
            close(g.shared_fd);
        g.shared_fd = -1;
    }
    VkExternalMemoryImageCreateInfo ext = {
        .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT};
    VkImageCreateInfo info = {.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                              .pNext = &ext,
                              .imageType = VK_IMAGE_TYPE_2D,
                              .format = VK_FORMAT_R8G8B8A8_UNORM,
                              .extent = {width, height, 1},
                              .mipLevels = 1,
                              .arrayLayers = 1,
                              .samples = VK_SAMPLE_COUNT_1_BIT,
                              .tiling = VK_IMAGE_TILING_OPTIMAL,
                              .usage = VK_IMAGE_USAGE_TRANSFER_SRC_BIT |
                                       VK_IMAGE_USAGE_TRANSFER_DST_BIT |
                                       VK_IMAGE_USAGE_SAMPLED_BIT,
                              .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
                              .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED};
    if (vkCreateImage(g.device, &info, NULL, &g.shared) != VK_SUCCESS)
        return 0;
    VkMemoryRequirements req;
    vkGetImageMemoryRequirements(g.device, g.shared, &req);
    g.shared_size = req.size;
    VkPhysicalDeviceMemoryProperties mem;
    vkGetPhysicalDeviceMemoryProperties(g.physical, &mem);
    uint32_t type = UINT32_MAX;
    for (uint32_t i = 0; i < mem.memoryTypeCount; i++) {
        if ((req.memoryTypeBits & (1u << i)) &&
            (mem.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
            type = i;
            break;
        }
    }
    if (type == UINT32_MAX)
        return 0;
    VkExportMemoryAllocateInfo export_mem = {
        .sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO,
        .pNext = NULL,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT};
    VkMemoryDedicatedAllocateInfo dedicated = {
        .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
        .pNext = &export_mem,
        .image = g.shared};
    VkMemoryAllocateInfo alloc = {.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                                  .pNext = &dedicated,
                                  .allocationSize = req.size,
                                  .memoryTypeIndex = type};
    if (vkAllocateMemory(g.device, &alloc, NULL, &g.shared_memory) != VK_SUCCESS)
        return 0;
    if (vkBindImageMemory(g.device, g.shared, g.shared_memory, 0) != VK_SUCCESS)
        return 0;

    PFN_vkGetMemoryFdKHR get_fd =
        (PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(g.device, "vkGetMemoryFdKHR");
    if (get_fd == NULL)
        return 0;
    VkMemoryGetFdInfoKHR fd_info = {.sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR,
                                    .memory = g.shared_memory,
                                    .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT};
    if (get_fd(g.device, &fd_info, &g.shared_fd) != VK_SUCCESS || g.shared_fd < 0)
        return 0;

    while (gl_get_error != NULL && gl_get_error() != GL_NO_ERROR)
        ;
    gl_gen_textures(1, &g.gl_texture);
    gl_bind_texture(GL_TEXTURE_2D, g.gl_texture);
    gl_create_memory_objects(1, &g.gl_memory);
    gl_import_memory_fd(g.gl_memory, (uint64_t)g.shared_size, GL_HANDLE_TYPE_OPAQUE_FD_EXT,
                        g.shared_fd);
    if (gl_get_error() != GL_NO_ERROR) {
        g.why = "OpenGL refuse la mémoire Vulkan (glImportMemoryFdEXT)";
        return 0;
    }
    gl_tex_storage_mem2d(GL_TEXTURE_2D, 1, GL_RGBA8, width, height, g.gl_memory, 0);
    if (gl_get_error() != GL_NO_ERROR) {
        g.why = "OpenGL refuse la taille de l'image (glTexStorageMem2DEXT)";
        return 0;
    }
    close(g.shared_fd);
    g.shared_fd = -1;
    g.width = width;
    g.height = height;
    return 1;
}

/* L'image partagée appartient à OpenGL : Vulkan la reprend depuis GENERAL pour
 * lire, et la rend à GENERAL pour la prochaine copie. */
static void barrier_shared(VkCommandBuffer cmd, VkImageLayout from, VkImageLayout to,
                           VkAccessFlags src, VkAccessFlags dst, VkPipelineStageFlags src_stage,
                           VkPipelineStageFlags dst_stage) {
    VkImageMemoryBarrier b = {.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                              .srcAccessMask = src,
                              .dstAccessMask = dst,
                              .oldLayout = from,
                              .newLayout = to,
                              .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
                              .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
                              .image = g.shared,
                              .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
    vkCmdPipelineBarrier(cmd, src_stage, dst_stage, 0, 0, NULL, 0, NULL, 1, &b);
}

static int copy_gl_to_shared(void) {
    GLint previous_texture = 0, previous_read = 0, previous_buffer = GL_BACK;
    gl_get_integerv(GL_TEXTURE_BINDING_2D, &previous_texture);
    gl_get_integerv(GL_READ_FRAMEBUFFER_BINDING, &previous_read);
    gl_get_integerv(GL_READ_BUFFER, &previous_buffer);
    gl_bind_texture(GL_TEXTURE_2D, g.gl_texture);
    if (previous_read != 0 && gl_bind_framebuffer != NULL)
        gl_bind_framebuffer(GL_READ_FRAMEBUFFER, 0); /* la fenêtre, jamais notre texture */
    gl_read_buffer(GL_BACK);
    gl_copy_tex_sub_image(GL_TEXTURE_2D, 0, 0, 0, 0, 0, (GLsizei)g.width, (GLsizei)g.height);
    int ok = gl_get_error() == GL_NO_ERROR;
    if (previous_read != 0 && gl_bind_framebuffer != NULL)
        gl_bind_framebuffer(GL_READ_FRAMEBUFFER, (GLuint)previous_read);
    gl_read_buffer((GLenum)previous_buffer);
    gl_bind_texture(GL_TEXTURE_2D, (GLuint)previous_texture);
    return ok;
}

static void destroy_vulkan(void);

/* La couche acquiert l'image de sa compagne avec un délai nul
 * (`AFMF_ACQUIRE_TIMEOUT_US=0`) : sous Xwayland, où le compositeur ne rend pas
 * une image dans l'instant, elle n'en obtient alors jamais. L'interposeur, lui,
 * sait qu'il présente par Vulkan et donne un budget par défaut mesuré (2 ms ne
 * rendaient une image que dans 2 % des cas, 8 ms dans ~25 %, sans retarder la
 * présentation). Une valeur déjà posée reste prioritaire. */
static void set_acquire_budget(void) {
    static int done;
    if (done)
        return;
    done = 1;
    if (getenv("AFMF_ACQUIRE_TIMEOUT_US") == NULL)
        setenv("AFMF_ACQUIRE_TIMEOUT_US", "8000", 0);
}

/* Les attentes du GPU sont bornées : un pilote qui ne rend pas la main doit
 * coûter le repli sur OpenGL natif, jamais le gel du jeu. */
static int wait_fence(uint64_t timeout_ns) {
    VkResult result = vkWaitForFences(g.device, 1, &g.fence, VK_TRUE, timeout_ns);
    if (result == VK_TIMEOUT) {
        g.why = "le GPU ne rend pas la main (attente de l'image trop longue)";
        return 0;
    }
    return result == VK_SUCCESS;
}

static int init_for(Display *dpy, Window win, uint32_t width, uint32_t height) {
    set_acquire_budget();
    g.dpy = dpy;
    g.win = win;
    if (!load_gl()) {
        g.why = "fonctions OpenGL de partage de mémoire absentes";
        return 0;
    }
    if (g.egl_wayland) {
        const char *exts[] = {VK_KHR_SURFACE_EXTENSION_NAME,
                              VK_KHR_WAYLAND_SURFACE_EXTENSION_NAME};
        if (!instance_has(VK_KHR_WAYLAND_SURFACE_EXTENSION_NAME)) {
            g.why = "VK_KHR_wayland_surface absent";
            return 0;
        }
        if (g.wl_surface == NULL) {
            g.why = "surface Wayland inconnue";
            return 0;
        }
        if (!vk_setup(exts, 2, &g.instance)) {
            g.why = "vkCreateInstance a échoué";
            return 0;
        }
        VkWaylandSurfaceCreateInfoKHR surface = {
            .sType = VK_STRUCTURE_TYPE_WAYLAND_SURFACE_CREATE_INFO_KHR,
            .display = (struct wl_display *)dpy,
            .surface = g.wl_surface};
        if (vkCreateWaylandSurfaceKHR(g.instance, &surface, NULL, &g.surface) != VK_SUCCESS) {
            g.why = "vkCreateWaylandSurfaceKHR a échoué";
            return 0;
        }
    } else {
        const char *exts[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_XLIB_SURFACE_EXTENSION_NAME};
        if (!instance_has(VK_KHR_XLIB_SURFACE_EXTENSION_NAME)) {
            g.why = "VK_KHR_xlib_surface absent";
            return 0;
        }
        if (!vk_setup(exts, 2, &g.instance)) {
            g.why = "vkCreateInstance a échoué";
            return 0;
        }
        VkXlibSurfaceCreateInfoKHR surface = {
            .sType = VK_STRUCTURE_TYPE_XLIB_SURFACE_CREATE_INFO_KHR, .dpy = dpy, .window = win};
        if (vkCreateXlibSurfaceKHR(g.instance, &surface, NULL, &g.surface) != VK_SUCCESS) {
            g.why = "vkCreateXlibSurfaceKHR a échoué";
            return 0;
        }
    }
    if (!pick_physical()) {
        g.why = "aucun appareil Vulkan capable de présenter sur cette fenêtre";
        return 0;
    }
    float priority = 1.0f;
    VkDeviceQueueCreateInfo queue = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                                     .queueFamilyIndex = g.family,
                                     .queueCount = 1,
                                     .pQueuePriorities = &priority};
    const char *device_exts[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME,
                                 VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME};
    VkDeviceCreateInfo device = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                                 .queueCreateInfoCount = 1,
                                 .pQueueCreateInfos = &queue,
                                 .enabledExtensionCount = 2,
                                 .ppEnabledExtensionNames = device_exts};
    if (vkCreateDevice(g.physical, &device, NULL, &g.device) != VK_SUCCESS) {
        g.why = "vkCreateDevice a échoué";
        return 0;
    }
    vkGetDeviceQueue(g.device, g.family, 0, &g.queue);

    VkCommandPoolCreateInfo pool = {.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
                                    .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
                                    .queueFamilyIndex = g.family};
    if (vkCreateCommandPool(g.device, &pool, NULL, &g.pool) != VK_SUCCESS)
        return 0;
    VkCommandBufferAllocateInfo alloc = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                                         .commandPool = g.pool,
                                         .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
                                         .commandBufferCount = 1};
    if (vkAllocateCommandBuffers(g.device, &alloc, &g.cmd) != VK_SUCCESS)
        return 0;
    VkFenceCreateInfo fence = {.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO,
                               .flags = VK_FENCE_CREATE_SIGNALED_BIT};
    if (vkCreateFence(g.device, &fence, NULL, &g.fence) != VK_SUCCESS)
        return 0;
    VkSemaphoreCreateInfo sem = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    if (vkCreateSemaphore(g.device, &sem, NULL, &g.sem_image) != VK_SUCCESS ||
        vkCreateSemaphore(g.device, &sem, NULL, &g.sem_drawn) != VK_SUCCESS)
        return 0;

    if (!create_swapchain(width, height, VK_NULL_HANDLE))
        return 0;
    if (!create_shared(g.extent.width, g.extent.height))
        return 0;

    /* L'image partagée est prise en charge par Vulkan juste une fois, au
     * départ : OpenGL la garde ensuite en GENERAL. */
    VkCommandBufferBeginInfo begin = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                                      .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
    vkBeginCommandBuffer(g.cmd, &begin);
    VkImageMemoryBarrier to_general = {
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
        .srcAccessMask = 0,
        .dstAccessMask = 0,
        .oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
        .newLayout = VK_IMAGE_LAYOUT_GENERAL,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .image = g.shared,
        .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
    vkCmdPipelineBarrier(g.cmd, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                         VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 1,
                         &to_general);
    vkEndCommandBuffer(g.cmd);
    VkSubmitInfo submit = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                           .commandBufferCount = 1,
                           .pCommandBuffers = &g.cmd};
    if (vkQueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS)
        return 0;
    /* Cinq secondes à l'initialisation : un premier appareil peut être lent,
     * mais pas indéfiniment. */
    if (!wait_fence(5000000000ull))
        return 0;
    vkResetFences(g.device, 1, &g.fence);
    return 1;
}

static void destroy_vulkan(void) {
    if (g.device != VK_NULL_HANDLE)
        vkDeviceWaitIdle(g.device);
    if (g.gl_texture != 0 && gl_delete_textures != NULL)
        gl_delete_textures(1, &g.gl_texture);
    if (g.gl_memory != 0 && gl_delete_memory_objects != NULL)
        gl_delete_memory_objects(1, &g.gl_memory);
    g.gl_texture = 0;
    g.gl_memory = 0;
    if (g.device != VK_NULL_HANDLE) {
        if (g.shared != VK_NULL_HANDLE)
            vkDestroyImage(g.device, g.shared, NULL);
        if (g.shared_memory != VK_NULL_HANDLE)
            vkFreeMemory(g.device, g.shared_memory, NULL);
        for (uint32_t i = 0; i < g.image_count; i++)
            if (g.views != NULL && g.views[i] != VK_NULL_HANDLE)
                vkDestroyImageView(g.device, g.views[i], NULL);
        if (g.swapchain != VK_NULL_HANDLE)
            vkDestroySwapchainKHR(g.device, g.swapchain, NULL);
        if (g.sem_drawn != VK_NULL_HANDLE)
            vkDestroySemaphore(g.device, g.sem_drawn, NULL);
        if (g.sem_image != VK_NULL_HANDLE)
            vkDestroySemaphore(g.device, g.sem_image, NULL);
        if (g.fence != VK_NULL_HANDLE)
            vkDestroyFence(g.device, g.fence, NULL);
        if (g.pool != VK_NULL_HANDLE)
            vkDestroyCommandPool(g.device, g.pool, NULL);
        vkDestroyDevice(g.device, NULL);
    }
    if (g.instance != VK_NULL_HANDLE && g.surface != VK_NULL_HANDLE)
        vkDestroySurfaceKHR(g.instance, g.surface, NULL);
    if (g.instance != VK_NULL_HANDLE)
        vkDestroyInstance(g.instance, NULL);
    if (g.shared_fd >= 0)
        close(g.shared_fd);
    free(g.images);
    free(g.views);
    memset(&g, 0, sizeof g);
    g.shared_fd = -1;
}

/* Recrée la chaîne et la texture partagée après un changement de taille. */
static int resize(uint32_t width, uint32_t height) {
    if (g.device == VK_NULL_HANDLE)
        return 0;
    vkDeviceWaitIdle(g.device);
    if (!create_swapchain(width, height, g.swapchain))
        return 0;
    if (!create_shared(g.extent.width, g.extent.height))
        return 0;
    VkCommandBufferBeginInfo begin = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                                      .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
    vkBeginCommandBuffer(g.cmd, &begin);
    VkImageMemoryBarrier to_general = {
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
        .srcAccessMask = 0,
        .dstAccessMask = 0,
        .oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
        .newLayout = VK_IMAGE_LAYOUT_GENERAL,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .image = g.shared,
        .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
    vkCmdPipelineBarrier(g.cmd, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                         VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 1,
                         &to_general);
    vkEndCommandBuffer(g.cmd);
    VkSubmitInfo submit = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                           .commandBufferCount = 1,
                           .pCommandBuffers = &g.cmd};
    if (vkQueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS || !wait_fence(2000000000ull))
        return 0;
    vkResetFences(g.device, 1, &g.fence);
    g.stats.resizes++;
    return 1;
}

/* Une image : copie OpenGL -> texture partagée, blit vers l'image de la
 * swapchain, présent. La couche afmf voit ce présent et place ses images
 * générées devant. */
static int present_frame(uint32_t width, uint32_t height) {
    double t0 = now_us();
    if (width != g.width || height != g.height) {
        if (!resize(width, height)) {
            g.why = "redimensionnement de la swapchain impossible";
            return 0;
        }
    }
    if (gl_finish != NULL)
        gl_finish();
    if (!copy_gl_to_shared()) {
        g.why = "copie du tampon OpenGL vers la mémoire partagée refusée";
        return 0;
    }
    /* La copie est dans le flux OpenGL ; Vulkan doit attendre qu'elle soit
     * finie avant de lire. `glFinish` coûte cher mais rend le partage sûr ;
     * c'est le prix de la version sans sémaphores GL_EXT_semaphore_fd. */
    if (gl_finish != NULL)
        gl_finish();
    double t_copy = now_us() - t0;

    if (!wait_fence(1000000000ull))
        return 0;
    uint32_t index = 0;
    VkResult acquired = vkAcquireNextImageKHR(g.device, g.swapchain, UINT64_MAX, g.sem_image,
                                              VK_NULL_HANDLE, &index);
    if (acquired == VK_ERROR_OUT_OF_DATE_KHR) {
        if (!resize(width, height))
            return 0;
        return 1;
    }
    if (acquired != VK_SUCCESS && acquired != VK_SUBOPTIMAL_KHR)
        return 0;
    vkResetFences(g.device, 1, &g.fence);
    vkResetCommandBuffer(g.cmd, 0);
    VkCommandBufferBeginInfo begin = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                                      .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
    vkBeginCommandBuffer(g.cmd, &begin);
    barrier_shared(g.cmd, VK_IMAGE_LAYOUT_GENERAL, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, 0,
                   VK_ACCESS_TRANSFER_READ_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                   VK_PIPELINE_STAGE_TRANSFER_BIT);
    VkImageMemoryBarrier to_dst = {
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
        .srcAccessMask = 0,
        .dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
        .oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
        .newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .image = g.images[index],
        .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
    vkCmdPipelineBarrier(g.cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL, 1, &to_dst);
    VkImageBlit region = {
        .srcSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1},
        .srcOffsets = {{0, 0, 0}, {(int32_t)g.width, (int32_t)g.height, 1}},
        .dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1},
        .dstOffsets = {{0, 0, 0},
                       {(int32_t)g.extent.width, (int32_t)g.extent.height, 1}}};
    vkCmdBlitImage(g.cmd, g.shared, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, g.images[index],
                   VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region, VK_FILTER_NEAREST);
    VkImageMemoryBarrier to_present = to_dst;
    to_present.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    to_present.dstAccessMask = 0;
    to_present.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    to_present.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    vkCmdPipelineBarrier(g.cmd, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, 0, 0, NULL, 0, NULL, 1,
                         &to_present);
    barrier_shared(g.cmd, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_IMAGE_LAYOUT_GENERAL,
                   VK_ACCESS_TRANSFER_READ_BIT, 0, VK_PIPELINE_STAGE_TRANSFER_BIT,
                   VK_PIPELINE_STAGE_ALL_COMMANDS_BIT);
    vkEndCommandBuffer(g.cmd);

    VkPipelineStageFlags wait_stage = VK_PIPELINE_STAGE_TRANSFER_BIT;
    VkSubmitInfo submit = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                           .waitSemaphoreCount = 1,
                           .pWaitSemaphores = &g.sem_image,
                           .pWaitDstStageMask = &wait_stage,
                           .commandBufferCount = 1,
                           .pCommandBuffers = &g.cmd,
                           .signalSemaphoreCount = 1,
                           .pSignalSemaphores = &g.sem_drawn};
    if (vkQueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS)
        return 0;
    VkPresentInfoKHR present = {.sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR,
                                .waitSemaphoreCount = 1,
                                .pWaitSemaphores = &g.sem_drawn,
                                .swapchainCount = 1,
                                .pSwapchains = &g.swapchain,
                                .pImageIndices = &index};
    VkResult presented = vkQueuePresentKHR(g.queue, &present);
    double t_present = now_us() - t0 - t_copy;
    g.stats.presents++;
    g.stats.copied++;
    g.stats.copy_us_total += t_copy;
    g.stats.present_us_total += t_present;
    if (presented == VK_ERROR_OUT_OF_DATE_KHR || presented == VK_SUBOPTIMAL_KHR) {
        /* La fenêtre a changé entre l'acquisition et le présent : on refait la
         * chaîne à la prochaine image, celle-ci est simplement perdue. */
        g.stats.resizes++;
        resize(width, height);
        return 1;
    }
    if (presented != VK_SUCCESS)
        return 0;
    if (g.stats.presents % 240 == 0)
        log_line("%lu images présentées, %.0f µs de copie, %.0f µs de présentation en moyenne",
                 g.stats.presents, g.stats.copy_us_total / (double)g.stats.copied,
                 g.stats.present_us_total / (double)g.stats.copied);
    return 1;
}

/* Vérifie une fois que les pixels de la texture partagée sont, côté Vulkan,
 * exactement ceux du tampon OpenGL : preuve qu'aucune image n'est perdue ni
 * mélangée dans le partage par fd. */
static void verify_once(void) {
    static int done;
    if (done || !g_log)
        return;
    done = 1;
    size_t size = (size_t)g.width * g.height * 4u;
    VkBufferCreateInfo info = {.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
                               .size = size,
                               .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT};
    VkBuffer buffer = VK_NULL_HANDLE;
    if (vkCreateBuffer(g.device, &info, NULL, &buffer) != VK_SUCCESS)
        return;
    VkMemoryRequirements req;
    vkGetBufferMemoryRequirements(g.device, buffer, &req);
    VkPhysicalDeviceMemoryProperties mem;
    vkGetPhysicalDeviceMemoryProperties(g.physical, &mem);
    uint32_t type = UINT32_MAX;
    for (uint32_t i = 0; i < mem.memoryTypeCount; i++)
        if ((req.memoryTypeBits & (1u << i)) &&
            (mem.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT) &&
            (mem.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) {
            type = i;
            break;
        }
    VkDeviceMemory memory = VK_NULL_HANDLE;
    if (type == UINT32_MAX)
        return;
    VkMemoryAllocateInfo alloc = {.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                                  .allocationSize = req.size,
                                  .memoryTypeIndex = type};
    vkAllocateMemory(g.device, &alloc, NULL, &memory);
    vkBindBufferMemory(g.device, buffer, memory, 0);
    VkCommandBufferBeginInfo begin = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                                      .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
    vkBeginCommandBuffer(g.cmd, &begin);
    barrier_shared(g.cmd, VK_IMAGE_LAYOUT_GENERAL, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, 0,
                   VK_ACCESS_TRANSFER_READ_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                   VK_PIPELINE_STAGE_TRANSFER_BIT);
    VkBufferImageCopy copy = {.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1},
                              .imageExtent = {g.width, g.height, 1}};
    vkCmdCopyImageToBuffer(g.cmd, g.shared, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, buffer, 1,
                           &copy);
    barrier_shared(g.cmd, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_IMAGE_LAYOUT_GENERAL,
                   VK_ACCESS_TRANSFER_READ_BIT, 0, VK_PIPELINE_STAGE_TRANSFER_BIT,
                   VK_PIPELINE_STAGE_ALL_COMMANDS_BIT);
    vkEndCommandBuffer(g.cmd);
    VkSubmitInfo submit = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                           .commandBufferCount = 1,
                           .pCommandBuffers = &g.cmd};
    if (vkQueueSubmit(g.queue, 1, &submit, g.fence) != VK_SUCCESS || !wait_fence(2000000000ull))
        return;
    vkResetFences(g.device, 1, &g.fence);
    void *mapped = NULL;
    if (vkMapMemory(g.device, memory, 0, size, 0, &mapped) == VK_SUCCESS) {
        unsigned char *vulkan = mapped;
        unsigned char *opengl = calloc(1, size);
        if (opengl != NULL) {
            gl_pixel_storei(GL_PACK_ALIGNMENT, 1);
            gl_read_pixels(0, 0, (GLsizei)g.width, (GLsizei)g.height, 0x1908 /* GL_RGBA */,
                           0x1401 /* GL_UNSIGNED_BYTE */, opengl);
            size_t different = 0;
            for (size_t i = 0; i < size; i += 4)
                if (vulkan[i] != opengl[i] || vulkan[i + 1] != opengl[i + 1] ||
                    vulkan[i + 2] != opengl[i + 2])
                    different++;
            log_line("vérification : %zu pixels différents sur %u (Vulkan relit ce qu'OpenGL a "
                     "dessiné)",
                     different, g.width * g.height);
            free(opengl);
        }
        vkUnmapMemory(g.device, memory);
    }
    vkDestroyBuffer(g.device, buffer, NULL);
    vkFreeMemory(g.device, memory, NULL);
}

/* --- entrées ------------------------------------------------------------- */

static int size_of_drawable(uint32_t *width, uint32_t *height) {
    if (p_glXQueryDrawable == NULL || g.dpy == NULL || g.win == 0)
        return 0;
    unsigned w = 0, h = 0;
    int ignored = 0;
    p_glXQueryDrawable(g.dpy, g.win, GLX_WIDTH, &w, &ignored);
    p_glXQueryDrawable(g.dpy, g.win, GLX_HEIGHT, &h, &ignored);
    if (w == 0 || h == 0)
        return 0;
    *width = w;
    *height = h;
    return 1;
}

static int ready(uint32_t width, uint32_t height) {
    if (!g.tried) {
        g.tried = 1;
        const char *flag = getenv("LAPINUX_GL_AFMF");
        if (flag != NULL && strcmp(flag, "0") == 0) {
            g.disabled = 1;
            return 0;
        }
        if (!init_for(g.dpy, g.win, width, height)) {
            g.disabled = 1;
            log_line("repli sur OpenGL natif : %s", g.why != NULL ? g.why : "initialisation");
            return 0;
        }
        const char *budget = getenv("AFMF_ACQUIRE_TIMEOUT_US");
        log_line("OpenGL présenté via Vulkan sur %s (%ux%u, couche afmf active, délai d'acquisition "
                 "%s µs)",
                 g.egl_wayland ? "Wayland" : "X11", g.extent.width, g.extent.height,
                 budget != NULL ? budget : "0");
        return 1;
    }
    return !g.disabled;
}

static void count_fallback(void) { g.stats.failed++; }

__attribute__((visibility("default"))) void glXSwapBuffers(Display *dpy, GLXDrawable drawable) {
    load_symbols();
    if (p_glXSwapBuffers == NULL)
        return;
    if (!g.disabled) {
        g.dpy = dpy;
        g.win = (Window)drawable;
        uint32_t width = 0, height = 0;
        if (!size_of_drawable(&width, &height)) {
            count_fallback();
            if (g.tried)
                log_line("repli : taille de la fenêtre inconnue");
            g.disabled = 1;
            p_glXSwapBuffers(dpy, drawable);
            return;
        }
        if (ready(width, height) && present_frame(width, height)) {
            if (getenv("LAPINUX_GL_AFMF_VERIFY") != NULL)
                verify_once();
            return;
        }
        if (!g.disabled) {
            count_fallback();
            g.disabled = 1;
            log_line("repli sur OpenGL natif : %s",
                     g.why != NULL ? g.why : "présentation Vulkan impossible");
        }
    }
    p_glXSwapBuffers(dpy, drawable);
}

__attribute__((visibility("default"))) EGLBoolean eglSwapBuffers(EGLDisplay display,
                                                                 EGLSurface surface) {
    load_symbols();
    if (p_eglSwapBuffers == NULL)
        return 1;
    int platform_ok = (g.egl_x11 && g.dpy != NULL && g.win != 0) ||
                      (g.egl_wayland && g.dpy != NULL && g.wl_surface != NULL);
    if (!g.disabled && platform_ok && p_eglQuerySurface != NULL) {
        EGLint w = 0, h = 0;
        p_eglQuerySurface(display, surface, EGL_WIDTH, &w);
        p_eglQuerySurface(display, surface, EGL_HEIGHT, &h);
        if (w > 0 && h > 0 && ready((uint32_t)w, (uint32_t)h) &&
            present_frame((uint32_t)w, (uint32_t)h)) {
            if (getenv("LAPINUX_GL_AFMF_VERIFY") != NULL)
                verify_once();
            return 1;
        }
        if (!g.disabled) {
            count_fallback();
            g.disabled = 1;
            log_line("repli sur OpenGL natif : %s",
                     g.why != NULL ? g.why : "présentation Vulkan impossible");
        }
    }
    return p_eglSwapBuffers(display, surface);
}

/* Sur X11, EGL ne donne pas le Display à la présentation : on le garde à
 * l'initialisation et à la création de la fenêtre. */
__attribute__((visibility("default"))) EGLDisplay eglGetPlatformDisplay(EGLenum platform,
                                                                        void *native,
                                                                        const EGLint *attribs) {
    load_symbols();
    if (p_eglGetPlatformDisplay == NULL)
        return NULL;
    EGLDisplay display = p_eglGetPlatformDisplay(platform, native, attribs);
    if (platform == EGL_PLATFORM_X11_KHR && native != NULL) {
        g.dpy = native;
        g.egl_display = display;
        g.egl_x11 = 1;
    } else if (platform == EGL_PLATFORM_WAYLAND_KHR && native != NULL) {
        g.dpy = native;
        g.egl_display = display;
        g.egl_wayland = 1;
        g.egl_x11 = 0;
    }
    return display;
}

__attribute__((visibility("default"))) EGLDisplay eglGetDisplay(void *native) {
    load_symbols();
    if (p_eglGetDisplay == NULL)
        return NULL;
    EGLDisplay display = p_eglGetDisplay(native);
    if (native != NULL) {
        g.dpy = native;
        g.egl_display = display;
        g.egl_x11 = 1;
    } else if (g.dpy == NULL && p_xOpenDisplay != NULL) {
        /* EGL_DEFAULT_DISPLAY sous X11 : la connexion n'est pas exposée, on en
         * ouvre une pour la surface Vulkan (même serveur X). */
        Display *dpy = p_xOpenDisplay(NULL);
        if (dpy != NULL) {
            g.dpy = dpy;
            g.closed_own_display = 1;
            g.egl_display = display;
            g.egl_x11 = 1;
        }
    }
    return display;
}

__attribute__((visibility("default"))) EGLSurface eglCreateWindowSurface(EGLDisplay display,
                                                                         EGLConfig config,
                                                                         void *native,
                                                                         const EGLint *attribs) {
    load_symbols();
    if (p_eglCreateWindowSurface == NULL)
        return NULL;
    EGLSurface surface = p_eglCreateWindowSurface(display, config, native, attribs);
    if (surface != NULL) {
        if (g.egl_wayland)
            g.wl_surface = native;
        else
            g.win = (Window)native;
        g.egl_surface = surface;
        g.egl_display = display;
    }
    return surface;
}

__attribute__((visibility("default"))) void *eglGetProcAddress(const char *name) {
    load_symbols();
    return p_eglGetProcAddress != NULL ? p_eglGetProcAddress(name) : NULL;
}

static void finish_interposer(void) {
    static int finished;
    if (finished)
        return;
    finished = 1;
    if (g.instance != VK_NULL_HANDLE || g.device != VK_NULL_HANDLE) {
        if (g_log && g.stats.presents > 0)
            log_line("bilan : %lu présents, %lu copies, %lu redimensionnements, %lu échecs",
                     g.stats.presents, g.stats.copied, g.stats.resizes, g.stats.failed);
        destroy_vulkan();
    }
    Display *own = g.closed_own_display ? g.dpy : NULL;
    if (own != NULL) {
        void (*close_display)(Display *) = NULL;
        void *x11 = dlopen("libX11.so.6", RTLD_NOW);
        if (x11 != NULL)
            *(void **)&close_display = dlsym(x11, "XCloseDisplay");
        if (close_display != NULL)
            close_display(own);
    }
}

__attribute__((destructor)) static void shutdown_interposer(void) {
    finish_interposer();
}

/* Le client de mesure appelle ceci juste avant _exit : le bilan et la
 * destruction Vulkan ont lieu, alors que le démontage des bibliothèques — celui
 * qui casse dans libxcb via le pilote NVIDIA en ×0 — est évité. */
__attribute__((visibility("default"))) void lapinux_gl_afmf_finish(void) {
    finish_interposer();
}
