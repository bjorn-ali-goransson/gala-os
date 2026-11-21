/* Direct3D 11 as a Windows program uses it (ROADMAP GPU G2): d3d11.dll
 * loaded at run time, a device and a swap chain on a window,
 * the back buffer cleared and a second render target copied into it,
 * the frame read back through a staging texture and presented.
 *
 * Expected values are Direct3D 11's documented ones; on gala the
 * implementation is DXVK over Vulkan on Mesa's lavapipe, which the
 * scenario expects by the adapter's name:
 *
 * - D3D11CreateDeviceAndSwapChain with D3D_DRIVER_TYPE_HARDWARE and
 *   feature level 11_0 succeeds and reports 11_0 or better; the DXGI
 *   adapter behind the device names the driver (printed).
 * - The swap chain's buffer 0 is an ID3D11Texture2D of the window's
 *   client size in DXGI_FORMAT_B8G8R8A8_UNORM; a render target view of
 *   it clears to (0.2, 0.4, 0.6).
 * - A second render target, half the client's size, cleared to
 *   (1.0, 0.8, 0.0) and copied into the middle of the back buffer
 *   (CopySubresourceRegion), puts a block there with no shader.
 * - Copied to a staging texture and mapped, the corner is 0x336699 and
 *   the middle 0xffcc00.
 * - A second device, created while the first is alive and its own
 *   feature level asked for (10_1, as a program that wants no more
 *   asks), succeeds and reports 10_1: a process may hold as many
 *   devices as it likes, and Word's startup holds two (ROADMAP item
 *   OF). A texture on the second device proves it works, not only that
 *   the call returned.
 * - Present(0, 0) puts the frame on the screen, which xtask checks; a
 *   click in the window ends the program (it waits up to 60 s for one:
 *   without one, a failure).
 *
 * Without d3d11.dll, or where it has no device (a Windows machine with
 * no GPU), it prints "d3d11_test: skipped" and exits with 0; gala's
 * scenario expects "d3d11_test: ok".
 *
 * Prints "d3d11_test: presented" once the frame is on the screen (xtask
 * then checks it and clicks), then "d3d11_test: ok", or exits with the
 * number of the failing check. */
#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <initguid.h>
#include <d3d11.h>
#include <dxgi.h>
#include <stdio.h>
#include <string.h>

static int check;
static HRESULT last;
#define CHECK(cond) do { check++; if (!(cond)) { printf("d3d11_test: check %d (line %d) failed (hr 0x%08lx)\n", check, __LINE__, (unsigned long)last); fflush(stdout); ExitProcess(check); } } while (0)
#define HR(call) do { last = (call); CHECK(SUCCEEDED(last)); } while (0)

static volatile LONG clicked;

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_LBUTTONUP:
        /* The click counts once the button is up: the program ends on it,
         * so it must not end before the release xtask sends with it (QEMU
         * gone before that write: "screen: Broken pipe" under KVM). */
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

typedef HRESULT(WINAPI *PFN_CREATE)(IDXGIAdapter *, D3D_DRIVER_TYPE, HMODULE, UINT,
                                    const D3D_FEATURE_LEVEL *, UINT, UINT,
                                    const DXGI_SWAP_CHAIN_DESC *, IDXGISwapChain **,
                                    ID3D11Device **, D3D_FEATURE_LEVEL *,
                                    ID3D11DeviceContext **);

typedef HRESULT(WINAPI *PFN_CREATE_DEVICE)(IDXGIAdapter *, D3D_DRIVER_TYPE, HMODULE, UINT,
                                           const D3D_FEATURE_LEVEL *, UINT, UINT,
                                           ID3D11Device **, D3D_FEATURE_LEVEL *,
                                           ID3D11DeviceContext **);

/* The pixel at (x, y) of a mapped B8G8R8A8 row of `pitch` bytes, as
 * 0xRRGGBB. */
static unsigned pixel(const D3D11_MAPPED_SUBRESOURCE *m, int x, int y) {
    const unsigned char *p = (const unsigned char *)m->pData + (size_t)y * m->RowPitch + (size_t)x * 4;
    return (unsigned)p[2] << 16 | (unsigned)p[1] << 8 | p[0];
}

int main(void) {
    HMODULE d3d11 = LoadLibraryW(L"d3d11.dll");
    if (!d3d11) {
        printf("d3d11_test: skipped (no d3d11.dll)\n");
        return 0;
    }
    PFN_CREATE create = (PFN_CREATE)(void *)GetProcAddress(d3d11, "D3D11CreateDeviceAndSwapChain");
    check++;
    if (!create) {
        printf("d3d11_test: skipped (no D3D11CreateDeviceAndSwapChain)\n");
        return 0;
    }

    HINSTANCE inst = GetModuleHandleW(NULL);
    WNDCLASSW wc;
    memset(&wc, 0, sizeof wc);
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorW(NULL, (LPCWSTR)IDC_ARROW);
    wc.lpszClassName = L"d3d11_test";
    CHECK(RegisterClassW(&wc) != 0);
    HWND hwnd = CreateWindowExW(0, L"d3d11_test", L"Direct3D 11", WS_OVERLAPPEDWINDOW, 100, 100, 400,
                                300, NULL, NULL, inst, NULL);
    CHECK(hwnd != NULL);
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    RECT client;
    CHECK(GetClientRect(hwnd, &client));
    UINT width = (UINT)(client.right - client.left);
    UINT height = (UINT)(client.bottom - client.top);
    CHECK(width >= 64 && height >= 64);

    DXGI_SWAP_CHAIN_DESC desc;
    memset(&desc, 0, sizeof desc);
    desc.BufferDesc.Width = width;
    desc.BufferDesc.Height = height;
    desc.BufferDesc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    desc.BufferCount = 2;
    desc.OutputWindow = hwnd;
    desc.Windowed = TRUE;
    desc.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;
    const D3D_FEATURE_LEVEL want[] = {D3D_FEATURE_LEVEL_11_0};
    IDXGISwapChain *swap = NULL;
    ID3D11Device *dev = NULL;
    ID3D11DeviceContext *ctx = NULL;
    D3D_FEATURE_LEVEL level = 0;
    last = create(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, want, 1, D3D11_SDK_VERSION, &desc, &swap,
                  &dev, &level, &ctx);
    if (FAILED(last)) {
        printf("d3d11_test: skipped (no device, hr 0x%08lx)\n", (unsigned long)last);
        return 0;
    }
    CHECK(swap && dev && ctx);
    CHECK(level >= D3D_FEATURE_LEVEL_11_0);

    /* The adapter behind the device names the driver. */
    IDXGIDevice *dxgi_dev = NULL;
    HR(ID3D11Device_QueryInterface(dev, &IID_IDXGIDevice, (void **)&dxgi_dev));
    IDXGIAdapter *adapter = NULL;
    HR(IDXGIDevice_GetAdapter(dxgi_dev, &adapter));
    DXGI_ADAPTER_DESC ad;
    HR(IDXGIAdapter_GetDesc(adapter, &ad));
    char name[128];
    size_t i;
    for (i = 0; i < sizeof name - 1 && ad.Description[i]; i++)
        name[i] = ad.Description[i] < 0x80 ? (char)ad.Description[i] : '?';
    name[i] = 0;
    printf("d3d11_test: device %s\n", name);
    fflush(stdout);

    ID3D11Texture2D *back = NULL;
    HR(IDXGISwapChain_GetBuffer(swap, 0, &IID_ID3D11Texture2D, (void **)&back));
    D3D11_TEXTURE2D_DESC bd;
    ID3D11Texture2D_GetDesc(back, &bd);
    CHECK(bd.Width == width && bd.Height == height);
    CHECK(bd.Format == DXGI_FORMAT_B8G8R8A8_UNORM);
    ID3D11RenderTargetView *rtv = NULL;
    HR(ID3D11Device_CreateRenderTargetView(dev, (ID3D11Resource *)back, NULL, &rtv));
    const float blue[4] = {0.2f, 0.4f, 0.6f, 1.0f};
    ID3D11DeviceContext_ClearRenderTargetView(ctx, rtv, blue);

    /* A second render target, cleared and copied into the middle of the
     * back buffer: a block with no shader. */
    D3D11_TEXTURE2D_DESC td;
    memset(&td, 0, sizeof td);
    td.Width = width / 2;
    td.Height = height / 2;
    td.MipLevels = 1;
    td.ArraySize = 1;
    td.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    td.SampleDesc.Count = 1;
    td.Usage = D3D11_USAGE_DEFAULT;
    td.BindFlags = D3D11_BIND_RENDER_TARGET;
    ID3D11Texture2D *block = NULL;
    HR(ID3D11Device_CreateTexture2D(dev, &td, NULL, &block));
    ID3D11RenderTargetView *block_rtv = NULL;
    HR(ID3D11Device_CreateRenderTargetView(dev, (ID3D11Resource *)block, NULL, &block_rtv));
    const float gold[4] = {1.0f, 0.8f, 0.0f, 1.0f};
    ID3D11DeviceContext_ClearRenderTargetView(ctx, block_rtv, gold);
    ID3D11DeviceContext_CopySubresourceRegion(ctx, (ID3D11Resource *)back, 0, width / 4, height / 4,
                                              0, (ID3D11Resource *)block, 0, NULL);

    /* Read back through a staging texture. */
    td.Width = width;
    td.Height = height;
    td.Usage = D3D11_USAGE_STAGING;
    td.BindFlags = 0;
    td.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    ID3D11Texture2D *staging = NULL;
    HR(ID3D11Device_CreateTexture2D(dev, &td, NULL, &staging));
    ID3D11DeviceContext_CopyResource(ctx, (ID3D11Resource *)staging, (ID3D11Resource *)back);
    D3D11_MAPPED_SUBRESOURCE mapped;
    HR(ID3D11DeviceContext_Map(ctx, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped));
    unsigned corner = pixel(&mapped, 4, 4);
    unsigned middle = pixel(&mapped, (int)width / 2, (int)height / 2);
    printf("d3d11_test: read back 0x%06x 0x%06x\n", corner, middle);
    fflush(stdout);
    ID3D11DeviceContext_Unmap(ctx, (ID3D11Resource *)staging, 0);
    CHECK(corner == 0x336699);
    CHECK(middle == 0xffcc00);

    /* A second device while the first is alive, which is what Word's
     * startup does: D3D11CreateDevice (no swap chain) with one feature
     * level asked for. */
    PFN_CREATE_DEVICE create_device =
        (PFN_CREATE_DEVICE)(void *)GetProcAddress(d3d11, "D3D11CreateDevice");
    CHECK(create_device != NULL);
    const D3D_FEATURE_LEVEL want_second[] = {D3D_FEATURE_LEVEL_10_1};
    ID3D11Device *dev2 = NULL;
    ID3D11DeviceContext *ctx2 = NULL;
    D3D_FEATURE_LEVEL level2 = 0;
    HR(create_device(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, want_second, 1, D3D11_SDK_VERSION,
                     &dev2, &level2, &ctx2));
    CHECK(dev2 && ctx2);
    CHECK(level2 == D3D_FEATURE_LEVEL_10_1);
    D3D11_TEXTURE2D_DESC sd;
    memset(&sd, 0, sizeof sd);
    sd.Width = 64;
    sd.Height = 64;
    sd.MipLevels = 1;
    sd.ArraySize = 1;
    sd.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    sd.SampleDesc.Count = 1;
    sd.Usage = D3D11_USAGE_DEFAULT;
    sd.BindFlags = D3D11_BIND_RENDER_TARGET;
    ID3D11Texture2D *second = NULL;
    HR(ID3D11Device_CreateTexture2D(dev2, &sd, NULL, &second));
    ID3D11Texture2D_Release(second);
    printf("d3d11_test: second device ok\n");
    fflush(stdout);

    HR(IDXGISwapChain_Present(swap, 0, 0));
    printf("d3d11_test: presented\n");
    fflush(stdout);

    SetTimer(hwnd, 1, 60000, NULL);
    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    CHECK(clicked);

    ID3D11DeviceContext_Release(ctx2);
    ID3D11Device_Release(dev2);
    ID3D11Texture2D_Release(staging);
    ID3D11RenderTargetView_Release(block_rtv);
    ID3D11Texture2D_Release(block);
    ID3D11RenderTargetView_Release(rtv);
    ID3D11Texture2D_Release(back);
    IDXGIAdapter_Release(adapter);
    IDXGIDevice_Release(dxgi_dev);
    ID3D11DeviceContext_Release(ctx);
    ID3D11Device_Release(dev);
    IDXGISwapChain_Release(swap);
    DestroyWindow(hwnd);
    printf("d3d11_test: ok\n");
    return 0;
}
