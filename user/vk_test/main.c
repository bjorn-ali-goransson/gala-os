/* Vulkan as a Windows program uses it (ROADMAP GPU G2): the Khronos loader
 * (vulkan-1.dll, loaded at run time as most programs do) finds the
 * installed driver through the registry, the program draws a triangle
 * into an image and reads it back, then presents the same frame to a
 * window through VK_KHR_win32_surface.
 *
 * Expected values are the Vulkan specification's (1.4) and what the
 * loader documents for Windows (LoaderDriverInterface.md: drivers listed
 * under HKLM\SOFTWARE\Khronos\Vulkan\Drivers); on gala the driver is
 * Mesa's lavapipe, which the scenario expects by name:
 *
 * - vkEnumerateInstanceVersion is at least 1.1; vkCreateInstance with
 *   VK_KHR_surface and VK_KHR_win32_surface succeeds; there is a
 *   physical device (its name, type and API version are printed) with a
 *   graphics queue family.
 * - vkCreateDevice (VK_KHR_swapchain) succeeds. A render pass clears a
 *   256 x 192 R8G8B8A8_UNORM image to (0.2, 0.4, 0.6) and draws a flat
 *   (1.0, 0.8, 0.0) triangle (SPIR-V from triangle.vert/.frag, no vertex
 *   buffer); copied to a host-visible buffer, the corner is 0x336699 and
 *   (0, 0.3) in normalized device coordinates (well inside the triangle)
 *   0xffcc00.
 * - A window's surface supports presenting from that queue family, has a
 *   B8G8R8A8 format (UNORM or SRGB) and FIFO; a swapchain of the
 *   window's client size gets the same frame (its pixels read back
 *   too), presented (VK_SUCCESS); xtask checks the window on the screen.
 * - A click in the window ends the program (it waits up to 60 s for one:
 *   without one, a failure); everything is destroyed in order.
 *
 * Without vulkan-1.dll, or with no Vulkan driver installed (a Windows
 * machine without a GPU driver), it prints "vk_test: skipped" and exits
 * with 0; gala's scenario expects "vk_test: ok".
 *
 * Prints "vk_test: presented" once the frame is on the screen (xtask then
 * checks it and clicks), then "vk_test: ok", or exits with the number of
 * the failing check. */
#define VK_NO_PROTOTYPES
#define VK_USE_PLATFORM_WIN32_KHR
#include <windows.h>
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <string.h>

#include "triangle_vert.h"
#include "triangle_frag.h"

static int check;
static VkResult last;
#define CHECK(cond) do { check++; if (!(cond)) { printf("vk_test: check %d (line %d) failed (VkResult %d)\n", check, __LINE__, (int)last); fflush(stdout); ExitProcess(check); } } while (0)
#define VK(call) do { last = (call); CHECK(last == VK_SUCCESS); } while (0)

#define WIDTH 256
#define HEIGHT 192

static PFN_vkGetInstanceProcAddr get_instance_proc;
static VkInstance instance;
static VkDevice device;

/* Instance and device functions, looked up as the loader documents. */
#define INSTANCE_FNS(X) \
    X(vkDestroyInstance) X(vkEnumeratePhysicalDevices) X(vkGetPhysicalDeviceProperties) \
    X(vkGetPhysicalDeviceQueueFamilyProperties) X(vkGetPhysicalDeviceMemoryProperties) \
    X(vkCreateDevice) X(vkGetDeviceProcAddr) X(vkCreateWin32SurfaceKHR) X(vkDestroySurfaceKHR) \
    X(vkGetPhysicalDeviceSurfaceSupportKHR) X(vkGetPhysicalDeviceSurfaceCapabilitiesKHR) \
    X(vkGetPhysicalDeviceSurfaceFormatsKHR) X(vkGetPhysicalDeviceSurfacePresentModesKHR)
#define DEVICE_FNS(X) \
    X(vkDestroyDevice) X(vkGetDeviceQueue) X(vkCreateImage) X(vkDestroyImage) \
    X(vkGetImageMemoryRequirements) X(vkAllocateMemory) X(vkFreeMemory) X(vkBindImageMemory) \
    X(vkCreateImageView) X(vkDestroyImageView) X(vkCreateBuffer) X(vkDestroyBuffer) \
    X(vkGetBufferMemoryRequirements) X(vkBindBufferMemory) X(vkMapMemory) X(vkUnmapMemory) \
    X(vkCreateRenderPass) X(vkDestroyRenderPass) X(vkCreateFramebuffer) X(vkDestroyFramebuffer) \
    X(vkCreateShaderModule) X(vkDestroyShaderModule) X(vkCreatePipelineLayout) \
    X(vkDestroyPipelineLayout) X(vkCreateGraphicsPipelines) X(vkDestroyPipeline) \
    X(vkCreateCommandPool) X(vkDestroyCommandPool) X(vkAllocateCommandBuffers) \
    X(vkBeginCommandBuffer) X(vkEndCommandBuffer) X(vkResetCommandBuffer) \
    X(vkCmdBeginRenderPass) X(vkCmdEndRenderPass) X(vkCmdBindPipeline) X(vkCmdDraw) \
    X(vkCmdCopyImageToBuffer) X(vkCmdPipelineBarrier) X(vkQueueSubmit) X(vkQueueWaitIdle) \
    X(vkCreateFence) X(vkDestroyFence) X(vkWaitForFences) X(vkResetFences) \
    X(vkCreateSemaphore) X(vkDestroySemaphore) X(vkDeviceWaitIdle) \
    X(vkCreateSwapchainKHR) X(vkDestroySwapchainKHR) X(vkGetSwapchainImagesKHR) \
    X(vkAcquireNextImageKHR) X(vkQueuePresentKHR)
#define DECLARE(f) static PFN_##f f;
INSTANCE_FNS(DECLARE)
DEVICE_FNS(DECLARE)

static PFN_vkVoidFunction instance_fn(const char *name) {
    PFN_vkVoidFunction f = get_instance_proc(instance, name);
    if (!f)
        printf("vk_test: no %s\n", name);
    CHECK(f != NULL);
    return f;
}

static PFN_vkVoidFunction device_fn(const char *name) {
    PFN_vkVoidFunction f = vkGetDeviceProcAddr(device, name);
    if (!f)
        printf("vk_test: no %s\n", name);
    CHECK(f != NULL);
    return f;
}

static VkPhysicalDeviceMemoryProperties memory_props;

/* A memory type in `bits` with all of `flags`. */
static uint32_t memory_type(uint32_t bits, VkMemoryPropertyFlags flags) {
    for (uint32_t i = 0; i < memory_props.memoryTypeCount; i++)
        if ((bits & (1u << i)) && (memory_props.memoryTypes[i].propertyFlags & flags) == flags)
            return i;
    CHECK(0);
    return 0;
}

/* What draws the triangle into images of one format. */
struct pipeline {
    VkRenderPass pass;
    VkPipelineLayout layout;
    VkPipeline pipe;
};

static VkShaderModule shader(const uint32_t *code, size_t size) {
    VkShaderModuleCreateInfo info = {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    info.codeSize = size;
    info.pCode = code;
    VkShaderModule m;
    VK(vkCreateShaderModule(device, &info, NULL, &m));
    return m;
}

/* A render pass that clears an image of `format` and leaves it ready
 * to be copied (TRANSFER_SRC_OPTIMAL), and the triangle's pipeline. */
static struct pipeline make_pipeline(VkFormat format, VkExtent2D extent) {
    struct pipeline p;
    VkAttachmentDescription color = {0};
    color.format = format;
    color.samples = VK_SAMPLE_COUNT_1_BIT;
    color.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
    color.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
    color.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
    color.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
    color.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    color.finalLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
    VkAttachmentReference ref = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
    VkSubpassDescription sub = {0};
    sub.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
    sub.colorAttachmentCount = 1;
    sub.pColorAttachments = &ref;
    VkSubpassDependency dep = {0};
    dep.srcSubpass = 0;
    dep.dstSubpass = VK_SUBPASS_EXTERNAL;
    dep.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    dep.dstStageMask = VK_PIPELINE_STAGE_TRANSFER_BIT;
    dep.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
    dep.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
    VkRenderPassCreateInfo rp = {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
    rp.attachmentCount = 1;
    rp.pAttachments = &color;
    rp.subpassCount = 1;
    rp.pSubpasses = &sub;
    rp.dependencyCount = 1;
    rp.pDependencies = &dep;
    VK(vkCreateRenderPass(device, &rp, NULL, &p.pass));

    VkPipelineLayoutCreateInfo li = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    VK(vkCreatePipelineLayout(device, &li, NULL, &p.layout));

    VkShaderModule vs = shader(vert_spv, sizeof vert_spv), fs = shader(frag_spv, sizeof frag_spv);
    VkPipelineShaderStageCreateInfo stages[2] = {{VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO},
                                                 {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO}};
    stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
    stages[0].module = vs;
    stages[0].pName = "main";
    stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
    stages[1].module = fs;
    stages[1].pName = "main";
    VkPipelineVertexInputStateCreateInfo vi = {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo ia = {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
    ia.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
    VkViewport viewport = {0, 0, (float)extent.width, (float)extent.height, 0, 1};
    VkRect2D scissor = {{0, 0}, extent};
    VkPipelineViewportStateCreateInfo vp = {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
    vp.viewportCount = 1;
    vp.pViewports = &viewport;
    vp.scissorCount = 1;
    vp.pScissors = &scissor;
    VkPipelineRasterizationStateCreateInfo rs = {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
    rs.polygonMode = VK_POLYGON_MODE_FILL;
    rs.cullMode = VK_CULL_MODE_NONE;
    rs.frontFace = VK_FRONT_FACE_CLOCKWISE;
    rs.lineWidth = 1.0f;
    VkPipelineMultisampleStateCreateInfo ms = {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
    ms.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
    VkPipelineColorBlendAttachmentState blend = {0};
    blend.colorWriteMask = 0xf;
    VkPipelineColorBlendStateCreateInfo cb = {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
    cb.attachmentCount = 1;
    cb.pAttachments = &blend;
    VkGraphicsPipelineCreateInfo gp = {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
    gp.stageCount = 2;
    gp.pStages = stages;
    gp.pVertexInputState = &vi;
    gp.pInputAssemblyState = &ia;
    gp.pViewportState = &vp;
    gp.pRasterizationState = &rs;
    gp.pMultisampleState = &ms;
    gp.pColorBlendState = &cb;
    gp.layout = p.layout;
    gp.renderPass = p.pass;
    VK(vkCreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &gp, NULL, &p.pipe));
    vkDestroyShaderModule(device, vs, NULL);
    vkDestroyShaderModule(device, fs, NULL);
    return p;
}

static void free_pipeline(struct pipeline *p) {
    vkDestroyPipeline(device, p->pipe, NULL);
    vkDestroyPipelineLayout(device, p->layout, NULL);
    vkDestroyRenderPass(device, p->pass, NULL);
}

static VkQueue queue;
static VkCommandBuffer cmd;
static VkFence fence;
static VkBuffer readback;
static VkDeviceMemory readback_mem;
static uint32_t *readback_pixels;

/* Records into `cmd`: the clear and the triangle on `image` (through
 * `view`), then its pixels copied to the readback buffer; if
 * `to_present`, the image left in PRESENT_SRC_KHR. Submits, waiting for
 * `wait` (if any) and signalling `done` (if any), and waits for the
 * fence. */
static void draw(struct pipeline *p, VkImage image, VkImageView view, VkExtent2D extent, int to_present,
                 VkSemaphore wait, VkSemaphore done) {
    VkFramebufferCreateInfo fi = {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
    fi.renderPass = p->pass;
    fi.attachmentCount = 1;
    fi.pAttachments = &view;
    fi.width = extent.width;
    fi.height = extent.height;
    fi.layers = 1;
    VkFramebuffer fb;
    VK(vkCreateFramebuffer(device, &fi, NULL, &fb));

    VK(vkResetCommandBuffer(cmd, 0));
    VkCommandBufferBeginInfo bi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    bi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    VK(vkBeginCommandBuffer(cmd, &bi));
    VkClearValue clear;
    clear.color.float32[0] = 0.2f;
    clear.color.float32[1] = 0.4f;
    clear.color.float32[2] = 0.6f;
    clear.color.float32[3] = 1.0f;
    VkRenderPassBeginInfo rb = {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
    rb.renderPass = p->pass;
    rb.framebuffer = fb;
    rb.renderArea.extent = extent;
    rb.clearValueCount = 1;
    rb.pClearValues = &clear;
    vkCmdBeginRenderPass(cmd, &rb, VK_SUBPASS_CONTENTS_INLINE);
    vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, p->pipe);
    vkCmdDraw(cmd, 3, 1, 0, 0);
    vkCmdEndRenderPass(cmd);
    VkBufferImageCopy copy = {0};
    copy.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    copy.imageSubresource.layerCount = 1;
    copy.imageExtent.width = extent.width;
    copy.imageExtent.height = extent.height;
    copy.imageExtent.depth = 1;
    vkCmdCopyImageToBuffer(cmd, image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, readback, 1, &copy);
    if (to_present) {
        VkImageMemoryBarrier b = {VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
        b.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
        b.dstAccessMask = 0;
        b.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
        b.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
        b.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        b.image = image;
        b.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
        b.subresourceRange.levelCount = 1;
        b.subresourceRange.layerCount = 1;
        vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, 0, 0,
                             NULL, 0, NULL, 1, &b);
    }
    VK(vkEndCommandBuffer(cmd));
    VkPipelineStageFlags stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    VkSubmitInfo si = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    si.waitSemaphoreCount = wait ? 1 : 0;
    si.pWaitSemaphores = &wait;
    si.pWaitDstStageMask = &stage;
    si.commandBufferCount = 1;
    si.pCommandBuffers = &cmd;
    si.signalSemaphoreCount = done ? 1 : 0;
    si.pSignalSemaphores = &done;
    VK(vkResetFences(device, 1, &fence));
    VK(vkQueueSubmit(queue, 1, &si, fence));
    VK(vkWaitForFences(device, 1, &fence, VK_TRUE, 30ull * 1000 * 1000 * 1000));
    vkDestroyFramebuffer(device, fb, NULL);
}

/* The read-back pixel at (x, y) of an image `width` wide, as 0xRRGGBB
 * (`bgra`: the image's bytes are B, G, R, A). */
static unsigned pixel(int x, int y, uint32_t width, int bgra) {
    const unsigned char *p = (const unsigned char *)&readback_pixels[y * width + x];
    return bgra ? (unsigned)p[2] << 16 | (unsigned)p[1] << 8 | p[0]
                : (unsigned)p[0] << 16 | (unsigned)p[1] << 8 | p[2];
}

static volatile LONG clicked;

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_LBUTTONDOWN:
        clicked = 1;
        PostQuitMessage(0);
        return 0;
    case WM_TIMER:
        PostQuitMessage(1);
        return 0;
    case WM_PAINT: {
        PAINTSTRUCT ps;
        BeginPaint(hwnd, &ps);
        EndPaint(hwnd, &ps);
        return 0;
    }
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

static void skipped(const char *why) {
    printf("vk_test: skipped (%s)\n", why);
    fflush(stdout);
    ExitProcess(0);
}

int main(void) {
    HMODULE loader = LoadLibraryW(L"vulkan-1.dll");
    if (!loader)
        skipped("no vulkan-1.dll");
    get_instance_proc = (PFN_vkGetInstanceProcAddr)(void *)GetProcAddress(loader, "vkGetInstanceProcAddr");
    CHECK(get_instance_proc != NULL);
    PFN_vkEnumerateInstanceVersion enum_version =
        (PFN_vkEnumerateInstanceVersion)get_instance_proc(NULL, "vkEnumerateInstanceVersion");
    PFN_vkCreateInstance create_instance = (PFN_vkCreateInstance)get_instance_proc(NULL, "vkCreateInstance");
    CHECK(enum_version != NULL && create_instance != NULL);
    uint32_t version = 0;
    VK(enum_version(&version));
    CHECK(version >= VK_API_VERSION_1_1);
    printf("vk_test: loader %u.%u.%u\n", VK_API_VERSION_MAJOR(version), VK_API_VERSION_MINOR(version),
           VK_API_VERSION_PATCH(version));

    VkApplicationInfo app = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    app.pApplicationName = "vk_test";
    app.apiVersion = VK_API_VERSION_1_1;
    const char *extensions[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_WIN32_SURFACE_EXTENSION_NAME};
    VkInstanceCreateInfo ici = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    ici.pApplicationInfo = &app;
    ici.enabledExtensionCount = 2;
    ici.ppEnabledExtensionNames = extensions;
    last = create_instance(&ici, NULL, &instance);
    if (last == VK_ERROR_INCOMPATIBLE_DRIVER || last == VK_ERROR_EXTENSION_NOT_PRESENT)
        skipped("no Vulkan driver");
    CHECK(last == VK_SUCCESS);
#define LOAD_INSTANCE(f) f = (PFN_##f)instance_fn(#f);
    INSTANCE_FNS(LOAD_INSTANCE)

    uint32_t count = 1;
    VkPhysicalDevice phys;
    last = vkEnumeratePhysicalDevices(instance, &count, &phys);
    CHECK((last == VK_SUCCESS || last == VK_INCOMPLETE) && count >= 1);
    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(phys, &props);
    static const char *const types[] = {"other", "integrated GPU", "discrete GPU", "virtual GPU", "CPU"};
    printf("vk_test: device %s (%s, Vulkan %u.%u)\n", props.deviceName,
           props.deviceType <= 4 ? types[props.deviceType] : "?", VK_API_VERSION_MAJOR(props.apiVersion),
           VK_API_VERSION_MINOR(props.apiVersion));
    fflush(stdout);
    vkGetPhysicalDeviceMemoryProperties(phys, &memory_props);
    VkQueueFamilyProperties families[16];
    uint32_t nfamilies = 16;
    vkGetPhysicalDeviceQueueFamilyProperties(phys, &nfamilies, families);
    uint32_t family = UINT32_MAX;
    for (uint32_t i = 0; i < nfamilies && family == UINT32_MAX; i++)
        if (families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)
            family = i;
    CHECK(family != UINT32_MAX);

    float priority = 1.0f;
    VkDeviceQueueCreateInfo qi = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
    qi.queueFamilyIndex = family;
    qi.queueCount = 1;
    qi.pQueuePriorities = &priority;
    const char *device_extensions[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME};
    VkDeviceCreateInfo di = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
    di.queueCreateInfoCount = 1;
    di.pQueueCreateInfos = &qi;
    di.enabledExtensionCount = 1;
    di.ppEnabledExtensionNames = device_extensions;
    VK(vkCreateDevice(phys, &di, NULL, &device));
#define LOAD_DEVICE(f) f = (PFN_##f)device_fn(#f);
    DEVICE_FNS(LOAD_DEVICE)
    vkGetDeviceQueue(device, family, 0, &queue);

    VkCommandPoolCreateInfo pi = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    pi.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pi.queueFamilyIndex = family;
    VkCommandPool pool;
    VK(vkCreateCommandPool(device, &pi, NULL, &pool));
    VkCommandBufferAllocateInfo ai = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    ai.commandPool = pool;
    ai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    ai.commandBufferCount = 1;
    VK(vkAllocateCommandBuffers(device, &ai, &cmd));
    VkFenceCreateInfo fci = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    VK(vkCreateFence(device, &fci, NULL, &fence));

    /* The readback buffer, big enough for the window's frame too. */
    VkBufferCreateInfo bci = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    bci.size = 2048 * 2048 * 4;
    bci.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    VK(vkCreateBuffer(device, &bci, NULL, &readback));
    VkMemoryRequirements req;
    vkGetBufferMemoryRequirements(device, readback, &req);
    VkMemoryAllocateInfo mai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    mai.allocationSize = req.size;
    mai.memoryTypeIndex = memory_type(req.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                                                              VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    VK(vkAllocateMemory(device, &mai, NULL, &readback_mem));
    VK(vkBindBufferMemory(device, readback, readback_mem, 0));
    VK(vkMapMemory(device, readback_mem, 0, VK_WHOLE_SIZE, 0, (void **)&readback_pixels));

    /* Offscreen: the triangle in an R8G8B8A8_UNORM image. */
    VkExtent2D extent = {WIDTH, HEIGHT};
    VkImageCreateInfo ii = {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    ii.imageType = VK_IMAGE_TYPE_2D;
    ii.format = VK_FORMAT_R8G8B8A8_UNORM;
    ii.extent.width = WIDTH;
    ii.extent.height = HEIGHT;
    ii.extent.depth = 1;
    ii.mipLevels = 1;
    ii.arrayLayers = 1;
    ii.samples = VK_SAMPLE_COUNT_1_BIT;
    ii.tiling = VK_IMAGE_TILING_OPTIMAL;
    ii.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
    VkImage image;
    VK(vkCreateImage(device, &ii, NULL, &image));
    vkGetImageMemoryRequirements(device, image, &req);
    mai.allocationSize = req.size;
    mai.memoryTypeIndex = memory_type(req.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    VkDeviceMemory image_mem;
    VK(vkAllocateMemory(device, &mai, NULL, &image_mem));
    VK(vkBindImageMemory(device, image, image_mem, 0));
    VkImageViewCreateInfo vci = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    vci.image = image;
    vci.viewType = VK_IMAGE_VIEW_TYPE_2D;
    vci.format = VK_FORMAT_R8G8B8A8_UNORM;
    vci.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    vci.subresourceRange.levelCount = 1;
    vci.subresourceRange.layerCount = 1;
    VkImageView view;
    VK(vkCreateImageView(device, &vci, NULL, &view));
    struct pipeline offscreen = make_pipeline(VK_FORMAT_R8G8B8A8_UNORM, extent);
    draw(&offscreen, image, view, extent, 0, VK_NULL_HANDLE, VK_NULL_HANDLE);
    unsigned corner = pixel(4, 4, WIDTH, 0), inside = pixel(WIDTH / 2, (int)(HEIGHT * 0.65), WIDTH, 0);
    printf("vk_test: read back 0x%06x 0x%06x\n", corner, inside);
    CHECK(corner == 0x336699);
    CHECK(inside == 0xffcc00);
    free_pipeline(&offscreen);
    vkDestroyImageView(device, view, NULL);
    vkDestroyImage(device, image, NULL);
    vkFreeMemory(device, image_mem, NULL);
    fflush(stdout);

    /* The same frame in a window: VK_KHR_win32_surface. */
    HINSTANCE inst = GetModuleHandleW(NULL);
    WNDCLASSW wc;
    memset(&wc, 0, sizeof wc);
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorW(NULL, (LPCWSTR)IDC_ARROW);
    wc.lpszClassName = L"vk_test";
    CHECK(RegisterClassW(&wc) != 0);
    HWND hwnd = CreateWindowExW(0, L"vk_test", L"Vulkan", WS_OVERLAPPEDWINDOW, 100, 100, 400, 300, NULL, NULL,
                                inst, NULL);
    CHECK(hwnd != NULL);
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    VkWin32SurfaceCreateInfoKHR sci = {VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR};
    sci.hinstance = inst;
    sci.hwnd = hwnd;
    VkSurfaceKHR surface;
    VK(vkCreateWin32SurfaceKHR(instance, &sci, NULL, &surface));
    VkBool32 supported = VK_FALSE;
    VK(vkGetPhysicalDeviceSurfaceSupportKHR(phys, family, surface, &supported));
    CHECK(supported);
    VkSurfaceCapabilitiesKHR caps;
    VK(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(phys, surface, &caps));
    RECT client;
    CHECK(GetClientRect(hwnd, &client));
    printf("vk_test: client %ldx%ld, surface %ux%u\n", client.right, client.bottom, caps.currentExtent.width,
           caps.currentExtent.height);
    CHECK(caps.currentExtent.width == (uint32_t)client.right && caps.currentExtent.height == (uint32_t)client.bottom);
    CHECK(caps.supportedUsageFlags & VK_IMAGE_USAGE_TRANSFER_SRC_BIT);
    VkSurfaceFormatKHR formats[32];
    uint32_t nformats = 32;
    VK(vkGetPhysicalDeviceSurfaceFormatsKHR(phys, surface, &nformats, formats));
    VkFormat format = VK_FORMAT_UNDEFINED;
    for (uint32_t i = 0; i < nformats && format == VK_FORMAT_UNDEFINED; i++)
        if (formats[i].format == VK_FORMAT_B8G8R8A8_UNORM)
            format = formats[i].format;
    CHECK(format != VK_FORMAT_UNDEFINED);
    VkPresentModeKHR modes[8];
    uint32_t nmodes = 8;
    VK(vkGetPhysicalDeviceSurfacePresentModesKHR(phys, surface, &nmodes, modes));
    int fifo = 0;
    for (uint32_t i = 0; i < nmodes; i++)
        fifo |= modes[i] == VK_PRESENT_MODE_FIFO_KHR;
    CHECK(fifo);

    VkSwapchainCreateInfoKHR sw = {VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR};
    sw.surface = surface;
    sw.minImageCount = caps.minImageCount;
    sw.imageFormat = format;
    sw.imageColorSpace = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
    sw.imageExtent = caps.currentExtent;
    sw.imageArrayLayers = 1;
    sw.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
    sw.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
    sw.preTransform = caps.currentTransform;
    sw.compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    sw.presentMode = VK_PRESENT_MODE_FIFO_KHR;
    sw.clipped = VK_TRUE;
    VkSwapchainKHR swapchain;
    VK(vkCreateSwapchainKHR(device, &sw, NULL, &swapchain));
    VkImage images[8];
    uint32_t nimages = 8;
    VK(vkGetSwapchainImagesKHR(device, swapchain, &nimages, images));
    CHECK(nimages >= 1);

    VkSemaphoreCreateInfo semi = {VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    VkSemaphore acquired, rendered;
    VK(vkCreateSemaphore(device, &semi, NULL, &acquired));
    VK(vkCreateSemaphore(device, &semi, NULL, &rendered));
    uint32_t index;
    VK(vkAcquireNextImageKHR(device, swapchain, UINT64_MAX, acquired, VK_NULL_HANDLE, &index));
    CHECK(index < nimages);
    vci.image = images[index];
    vci.format = format;
    VK(vkCreateImageView(device, &vci, NULL, &view));
    struct pipeline onscreen = make_pipeline(format, caps.currentExtent);
    draw(&onscreen, images[index], view, caps.currentExtent, 1, acquired, rendered);
    uint32_t w = caps.currentExtent.width, h = caps.currentExtent.height;
    corner = pixel(4, 4, w, 1);
    inside = pixel(w / 2, (int)(h * 0.65), w, 1);
    printf("vk_test: swapchain image read back 0x%06x 0x%06x\n", corner, inside);
    CHECK(corner == 0x336699);
    CHECK(inside == 0xffcc00);
    VkPresentInfoKHR present = {VK_STRUCTURE_TYPE_PRESENT_INFO_KHR};
    present.waitSemaphoreCount = 1;
    present.pWaitSemaphores = &rendered;
    present.swapchainCount = 1;
    present.pSwapchains = &swapchain;
    present.pImageIndices = &index;
    VK(vkQueuePresentKHR(queue, &present));
    VK(vkQueueWaitIdle(queue));
    printf("vk_test: presented\n");
    fflush(stdout);

    SetTimer(hwnd, 1, 60000, NULL);
    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    CHECK(clicked);

    VK(vkDeviceWaitIdle(device));
    vkDestroySemaphore(device, acquired, NULL);
    vkDestroySemaphore(device, rendered, NULL);
    free_pipeline(&onscreen);
    vkDestroyImageView(device, view, NULL);
    vkDestroySwapchainKHR(device, swapchain, NULL);
    vkDestroySurfaceKHR(instance, surface, NULL);
    vkUnmapMemory(device, readback_mem);
    vkDestroyBuffer(device, readback, NULL);
    vkFreeMemory(device, readback_mem, NULL);
    vkDestroyFence(device, fence, NULL);
    vkDestroyCommandPool(device, pool, NULL);
    vkDestroyDevice(device, NULL);
    vkDestroyInstance(instance, NULL);
    DestroyWindow(hwnd);
    printf("vk_test: ok\n");
    return 0;
}
