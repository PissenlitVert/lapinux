// Vérifie qu'un périphérique Vulkan peut encore être créé sur cette machine.
//
// Le pilote NVIDIA peut laisser le GPU en NV_ERR_RESET_REQUIRED après un
// plantage : l'énumération marche encore, mais vkCreateDevice échoue pour tout
// le monde. Le script de validation s'en sert comme précondition, pour ne pas
// confondre « la génération ne marche pas » et « le GPU demande un reset ».
//
// Usage : vulkan-probe        code 0 = appareil créé, 1 = refus
// Compilation : scripts/build-gl-interpose.sh

#include <vulkan/vulkan.h>

#include <stdio.h>
#include <string.h>

int main(void) {
    VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
                             .apiVersion = VK_API_VERSION_1_1};
    VkInstanceCreateInfo info = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                                 .pApplicationInfo = &app};
    VkInstance instance = VK_NULL_HANDLE;
    if (vkCreateInstance(&info, NULL, &instance) != VK_SUCCESS) {
        printf("périphérique Vulkan : ÉCHEC (instance impossible)\n");
        return 1;
    }
    uint32_t count = 0;
    vkEnumeratePhysicalDevices(instance, &count, NULL);
    VkPhysicalDevice devices[8];
    if (count > 8)
        count = 8;
    vkEnumeratePhysicalDevices(instance, &count, devices);
    for (uint32_t i = 0; i < count; i++) {
        VkPhysicalDeviceProperties props;
        vkGetPhysicalDeviceProperties(devices[i], &props);
        if (props.deviceType == VK_PHYSICAL_DEVICE_TYPE_CPU)
            continue;
        uint32_t families = 0;
        vkGetPhysicalDeviceQueueFamilyProperties(devices[i], &families, NULL);
        VkQueueFamilyProperties fams[16];
        if (families > 16)
            families = 16;
        vkGetPhysicalDeviceQueueFamilyProperties(devices[i], &families, fams);
        uint32_t family = UINT32_MAX;
        for (uint32_t f = 0; f < families; f++)
            if (fams[f].queueFlags & VK_QUEUE_GRAPHICS_BIT) {
                family = f;
                break;
            }
        if (family == UINT32_MAX)
            continue;
        float priority = 1.0f;
        VkDeviceQueueCreateInfo queue = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                                         .queueFamilyIndex = family,
                                         .queueCount = 1,
                                         .pQueuePriorities = &priority};
        VkDeviceCreateInfo device = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                                     .queueCreateInfoCount = 1,
                                     .pQueueCreateInfos = &queue};
        VkDevice handle = VK_NULL_HANDLE;
        VkResult result = vkCreateDevice(devices[i], &device, NULL, &handle);
        if (result == VK_SUCCESS) {
            printf("périphérique Vulkan : OK (%s)\n", props.deviceName);
            vkDestroyDevice(handle, NULL);
            vkDestroyInstance(instance, NULL);
            return 0;
        }
        printf("périphérique Vulkan : ÉCHEC sur %s (vkCreateDevice = %d)\n", props.deviceName,
               (int)result);
        vkDestroyInstance(instance, NULL);
        return 1;
    }
    printf("périphérique Vulkan : ÉCHEC (aucun appareil graphique)\n");
    vkDestroyInstance(instance, NULL);
    return 1;
}
