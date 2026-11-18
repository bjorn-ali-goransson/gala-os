/* Direct2D 1.1 on a program's own Direct3D 11 device (ROADMAP OF, GPU
 * G2), the path Word draws its document with: a device context whose
 * target is a bitmap over the swap chain's DXGI surface.
 *
 * Expected values are Direct2D's documented ones; on gala the Direct3D
 * 11 device is DXVK over Vulkan on Mesa's lavapipe, which the scenario
 * expects by the adapter's name, and the Direct2D above it is gala's own
 * d2d1.dll, drawing in software.
 *
 * - D2D1CreateFactory for IID_ID2D1Factory1 succeeds, and the factory it
 *   gives is an ID2D1Factory as well (one object, two interfaces).
 * - ID2D1Factory1::CreateDevice over the Direct3D device's IDXGIDevice
 *   gives an ID2D1Device, and ID2D1Device::CreateDeviceContext a context
 *   that is an ID2D1RenderTarget as well; GetDevice gives the device
 *   back.
 * - The swap chain's buffer 0 as an IDXGISurface makes a target bitmap
 *   (CreateBitmapFromDxgiSurface, B8G8R8A8 premultiplied,
 *   TARGET | CANNOT_DRAW): its pixel size is the window's client size,
 *   GetOptions reports what it was made with and GetSurface gives the
 *   same surface back. Before SetTarget a context has no target; after
 *   it, GetTarget gives the bitmap.
 * - A frame drawn into it -- Clear(0.2, 0.4, 0.6) and a rectangle of
 *   (1.0, 0.8, 0.0) over the client's middle half -- ends with EndDraw
 *   S_OK and reaches the surface: copied to a staging texture and
 *   mapped, the corner is 0x336699 and the middle 0xffcc00 (the colours
 *   and the geometry of d3d11_test's frame, drawn by Direct2D instead of
 *   by Direct3D's clears and copies).
 * - Present(0, 0) puts the frame on the screen, which xtask checks; a
 *   click in the window ends the program (it waits up to 60 s for one:
 *   without one, a failure).
 *
 * Pitfall: a Direct2D method that returns a structure (GetPixelSize,
 * GetSize, GetPixelFormat) returns it through a hidden pointer, as the
 * C++ ABI returns structures from methods -- which is what Direct2D's
 * C++ declarations say and what the implementations do. mingw-w64's C
 * declarations of those three say by value instead, so GetPixelSize is
 * called here through a pointer of our own.
 *
 * Without d3d11.dll, or where it has no device (a Windows machine with
 * no GPU), it prints "d2d1_device_test: skipped" and exits with 0;
 * gala's scenario expects "d2d1_device_test: ok". */
#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <initguid.h>
#include <d3d11.h>
#include <dxgi.h>
#include <d2d1.h>
#include <d2d1_1.h>
#include <stdio.h>
#include <string.h>

static int check;
static HRESULT last;
#define CHECK(cond) do { check++; if (!(cond)) { printf("d2d1_device_test: check %d (line %d) failed (hr 0x%08lx)\n", check, __LINE__, (unsigned long)last); fflush(stdout); ExitProcess(check); } } while (0)
#define HR(call) do { last = (call); CHECK(SUCCEEDED(last)); } while (0)

static volatile LONG clicked;

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_LBUTTONUP:
        /* The click counts once the button is up: the program ends on it,
         * so it must not end before the release xtask sends with it. */
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
typedef HRESULT(WINAPI *PFN_D2D1_CREATE_FACTORY)(D2D1_FACTORY_TYPE, REFIID,
                                                 const D2D1_FACTORY_OPTIONS *, void **);

/* GetPixelSize through the hidden pointer the ABI uses (see the pitfall
 * above): slot 5 of ID2D1Bitmap (IUnknown's three, GetFactory,
 * GetSize). */
typedef D2D1_SIZE_U *(STDMETHODCALLTYPE *PFN_GET_PIXEL_SIZE)(void *, D2D1_SIZE_U *);

static D2D1_SIZE_U pixel_size(void *bitmap) {
    void **vtbl = *(void ***)bitmap;
    D2D1_SIZE_U size;
    size.width = 0;
    size.height = 0;
    ((PFN_GET_PIXEL_SIZE)vtbl[5])(bitmap, &size);
    return size;
}

/* QueryInterface and Release are slots 0 and 2 of every interface;
 * the headers' C declarations reach them through a chain of nested
 * Base members whose depth differs per interface. */
static HRESULT qi(void *o, REFIID iid, void **out) {
    IUnknown *u = (IUnknown *)o;
    return u->lpVtbl->QueryInterface(u, iid, out);
}

static void rel(void *o) {
    IUnknown *u = (IUnknown *)o;
    u->lpVtbl->Release(u);
}

/* The pixel at (x, y) of a mapped B8G8R8A8 row, as 0xRRGGBB. */
static unsigned pixel(const D3D11_MAPPED_SUBRESOURCE *m, int x, int y) {
    const unsigned char *p = (const unsigned char *)m->pData + (size_t)y * m->RowPitch + (size_t)x * 4;
    return (unsigned)p[2] << 16 | (unsigned)p[1] << 8 | p[0];
}

int main(void) {
    HMODULE d3d11 = LoadLibraryW(L"d3d11.dll");
    if (!d3d11) {
        printf("d2d1_device_test: skipped (no d3d11.dll)\n");
        return 0;
    }
    PFN_CREATE create = (PFN_CREATE)(void *)GetProcAddress(d3d11, "D3D11CreateDeviceAndSwapChain");
    if (!create) {
        printf("d2d1_device_test: skipped (no D3D11CreateDeviceAndSwapChain)\n");
        return 0;
    }
    HMODULE d2d1 = LoadLibraryW(L"d2d1.dll");
    if (!d2d1) {
        printf("d2d1_device_test: skipped (no d2d1.dll)\n");
        return 0;
    }
    PFN_D2D1_CREATE_FACTORY d2d1_create =
        (PFN_D2D1_CREATE_FACTORY)(void *)GetProcAddress(d2d1, "D2D1CreateFactory");
    if (!d2d1_create) {
        printf("d2d1_device_test: skipped (no D2D1CreateFactory)\n");
        return 0;
    }

    HINSTANCE inst = GetModuleHandleW(NULL);
    WNDCLASSW wc;
    memset(&wc, 0, sizeof wc);
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorW(NULL, (LPCWSTR)IDC_ARROW);
    wc.lpszClassName = L"d2d1_device_test";
    CHECK(RegisterClassW(&wc) != 0);
    HWND hwnd = CreateWindowExW(0, L"d2d1_device_test", L"Direct2D on Direct3D 11",
                                WS_OVERLAPPEDWINDOW, 100, 100, 400, 300, NULL, NULL, inst, NULL);
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
    last = create(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, D3D11_CREATE_DEVICE_BGRA_SUPPORT, want, 1,
                  D3D11_SDK_VERSION, &desc, &swap, &dev, &level, &ctx);
    if (FAILED(last)) {
        printf("d2d1_device_test: skipped (no device, hr 0x%08lx)\n", (unsigned long)last);
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
    printf("d2d1_device_test: device %s\n", name);
    fflush(stdout);

    /* A Direct2D 1.1 factory, and a Direct2D device over the program's
     * own Direct3D device. */
    ID2D1Factory1 *factory = NULL;
    HR(d2d1_create(D2D1_FACTORY_TYPE_SINGLE_THREADED, &IID_ID2D1Factory1, NULL, (void **)&factory));
    CHECK(factory != NULL);
    ID2D1Factory *factory0 = NULL;
    HR(qi(factory, &IID_ID2D1Factory, (void **)&factory0));
    CHECK(factory0 != NULL);
    rel(factory0);

    ID2D1Device *d2d_dev = NULL;
    HR(factory->lpVtbl->CreateDevice(factory, dxgi_dev, &d2d_dev));
    CHECK(d2d_dev != NULL);
    ID2D1DeviceContext *dc = NULL;
    HR(d2d_dev->lpVtbl->CreateDeviceContext(d2d_dev, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, &dc));
    CHECK(dc != NULL);
    ID2D1RenderTarget *rt = NULL;
    HR(qi(dc, &IID_ID2D1RenderTarget, (void **)&rt));
    CHECK(rt != NULL);
    rel(rt);
    ID2D1Device *same = NULL;
    dc->lpVtbl->GetDevice(dc, &same);
    CHECK(same == d2d_dev);
    if (same)
        rel(same);

    /* The swap chain's back buffer as a Direct2D target. */
    IDXGISurface *surface = NULL;
    HR(IDXGISwapChain_GetBuffer(swap, 0, &IID_IDXGISurface, (void **)&surface));
    D2D1_BITMAP_PROPERTIES1 props;
    memset(&props, 0, sizeof props);
    props.pixelFormat.format = DXGI_FORMAT_B8G8R8A8_UNORM;
    props.pixelFormat.alphaMode = D2D1_ALPHA_MODE_PREMULTIPLIED;
    props.dpiX = 96.0f;
    props.dpiY = 96.0f;
    props.bitmapOptions = D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW;
    ID2D1Bitmap1 *bitmap = NULL;
    HR(dc->lpVtbl->CreateBitmapFromDxgiSurface(dc, surface, &props, &bitmap));
    CHECK(bitmap != NULL);
    D2D1_SIZE_U size = pixel_size(bitmap);
    CHECK(size.width == width && size.height == height);
    CHECK(bitmap->lpVtbl->GetOptions(bitmap) ==
          (D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW));
    IDXGISurface *back_again = NULL;
    HR(bitmap->lpVtbl->GetSurface(bitmap, &back_again));
    CHECK(back_again == surface);
    if (back_again)
        IDXGISurface_Release(back_again);

    struct ID2D1Image *target = (struct ID2D1Image *)(void *)1;
    dc->lpVtbl->GetTarget(dc, &target);
    CHECK(target == NULL);
    dc->lpVtbl->SetTarget(dc, (struct ID2D1Image *)bitmap);
    dc->lpVtbl->GetTarget(dc, &target);
    CHECK(target == (struct ID2D1Image *)bitmap);
    if (target)
        rel(target);

    /* The frame: a cleared background and a rectangle over the client's
     * middle half, the colours and the geometry of d3d11_test's. */
    const D2D1_COLOR_F blue = {0.2f, 0.4f, 0.6f, 1.0f};
    const D2D1_COLOR_F gold = {1.0f, 0.8f, 0.0f, 1.0f};
    ID2D1SolidColorBrush *brush = NULL;
    dc->lpVtbl->Base.BeginDraw((ID2D1RenderTarget *)dc);
    dc->lpVtbl->Base.Clear((ID2D1RenderTarget *)dc, &blue);
    HR(dc->lpVtbl->Base.CreateSolidColorBrush((ID2D1RenderTarget *)dc, &gold, NULL, &brush));
    D2D1_RECT_F rect;
    rect.left = (FLOAT)(width / 4);
    rect.top = (FLOAT)(height / 4);
    rect.right = rect.left + (FLOAT)(width / 2);
    rect.bottom = rect.top + (FLOAT)(height / 2);
    dc->lpVtbl->Base.FillRectangle((ID2D1RenderTarget *)dc, &rect, (ID2D1Brush *)brush);
    HR(dc->lpVtbl->Base.EndDraw((ID2D1RenderTarget *)dc, NULL, NULL));

    /* Read the surface back through a staging texture: what Direct2D
     * drew has to be in the swap chain's buffer, not only in Direct2D. */
    ID3D11Texture2D *back = NULL;
    HR(IDXGISwapChain_GetBuffer(swap, 0, &IID_ID3D11Texture2D, (void **)&back));
    D3D11_TEXTURE2D_DESC td;
    memset(&td, 0, sizeof td);
    td.Width = width;
    td.Height = height;
    td.MipLevels = 1;
    td.ArraySize = 1;
    td.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    td.SampleDesc.Count = 1;
    td.Usage = D3D11_USAGE_STAGING;
    td.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    ID3D11Texture2D *staging = NULL;
    HR(ID3D11Device_CreateTexture2D(dev, &td, NULL, &staging));
    ID3D11DeviceContext_CopyResource(ctx, (ID3D11Resource *)staging, (ID3D11Resource *)back);
    D3D11_MAPPED_SUBRESOURCE mapped;
    HR(ID3D11DeviceContext_Map(ctx, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped));
    unsigned corner = pixel(&mapped, 4, 4);
    unsigned middle = pixel(&mapped, (int)width / 2, (int)height / 2);
    printf("d2d1_device_test: read back 0x%06x 0x%06x\n", corner, middle);
    fflush(stdout);
    ID3D11DeviceContext_Unmap(ctx, (ID3D11Resource *)staging, 0);
    CHECK(corner == 0x336699);
    CHECK(middle == 0xffcc00);

    HR(IDXGISwapChain_Present(swap, 0, 0));
    printf("d2d1_device_test: presented\n");
    fflush(stdout);

    SetTimer(hwnd, 1, 60000, NULL);
    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    CHECK(clicked);

    ID3D11Texture2D_Release(staging);
    ID3D11Texture2D_Release(back);
    if (brush)
        rel(brush);
    rel(bitmap);
    IDXGISurface_Release(surface);
    rel(dc);
    rel(d2d_dev);
    rel(factory);
    IDXGIAdapter_Release(adapter);
    IDXGIDevice_Release(dxgi_dev);
    ID3D11DeviceContext_Release(ctx);
    ID3D11Device_Release(dev);
    IDXGISwapChain_Release(swap);
    DestroyWindow(hwnd);
    printf("d2d1_device_test: ok\n");
    return 0;
}
