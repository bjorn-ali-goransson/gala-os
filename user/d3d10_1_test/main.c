/* Direct3D 10.1 and Direct2D 1.0 over it (ROADMAP OF, GPU G2), the path
 * Word takes when it does not use Direct2D's device: a render target
 * over a texture's DXGI surface, made by
 * ID2D1Factory::CreateDxgiSurfaceRenderTarget.
 *
 * Expected values are Direct3D's and Direct2D's documented ones; on gala
 * the Direct3D 10.1 device is gala's own d3d10_1.dll over the program's
 * Direct3D 11 (DXVK over Vulkan on Mesa's lavapipe, which the scenario
 * expects by the adapter's name) and the Direct2D above it is gala's own
 * d2d1.dll, drawing in software.
 *
 * - D3D10CreateDevice1 with D3D10_FEATURE_LEVEL_10_1 gives a device
 *   whose GetFeatureLevel reports at least 10_1, and which answers
 *   IID_IDXGIDevice: the adapter behind it names the driver.
 * - CreateTexture2D makes a 256 x 256 B8G8R8A8 render target, and that
 *   texture answers IID_IDXGISurface; GetDesc reports what it was made
 *   with, and GetDevice gives the device back.
 * - ID2D1Factory::CreateDxgiSurfaceRenderTarget over that surface gives
 *   a render target (B8G8R8A8 premultiplied, default DPI), whose frame
 *   -- Clear(0.2, 0.4, 0.6) and a rectangle of (1.0, 0.8, 0.0) over the
 *   middle half -- ends with EndDraw S_OK and reaches the texture:
 *   copied to a staging texture and mapped, the corner is 0x336699 and
 *   the middle 0xffcc00 (the colours and the geometry of
 *   d2d1_device_test's frame, drawn onto a Direct3D 10 texture instead
 *   of a swap chain's surface).
 *
 * Without d3d10_1.dll, or where it has no device (a machine with no
 * GPU), it prints "d3d10_1_test: skipped" and exits with 0; gala's
 * scenario expects "d3d10_1_test: ok". */
#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <initguid.h>
#include <d3d10_1.h>
#include <dxgi.h>
#include <d2d1.h>
#include <stdio.h>
#include <string.h>

static int check;
static HRESULT last;
#define CHECK(cond) do { check++; if (!(cond)) { printf("d3d10_1_test: check %d (line %d) failed (hr 0x%08lx)\n", check, __LINE__, (unsigned long)last); fflush(stdout); ExitProcess(check); } } while (0)
#define HR(call) do { last = (call); CHECK(SUCCEEDED(last)); } while (0)

#define SIDE 256

typedef HRESULT(WINAPI *PFN_CREATE1)(IDXGIAdapter *, D3D10_DRIVER_TYPE, HMODULE, UINT,
                                     D3D10_FEATURE_LEVEL1, UINT, ID3D10Device1 **);
typedef HRESULT(WINAPI *PFN_D2D1_CREATE_FACTORY)(D2D1_FACTORY_TYPE, REFIID,
                                                 const D2D1_FACTORY_OPTIONS *, void **);

static HRESULT qi(void *o, REFIID iid, void **out) {
    IUnknown *u = (IUnknown *)o;
    return u->lpVtbl->QueryInterface(u, iid, out);
}

static void rel(void *o) {
    IUnknown *u = (IUnknown *)o;
    u->lpVtbl->Release(u);
}

/* The pixel at (x, y) of a mapped B8G8R8A8 row, as 0xRRGGBB. */
static unsigned pixel(const D3D10_MAPPED_TEXTURE2D *m, int x, int y) {
    const unsigned char *p = (const unsigned char *)m->pData + (size_t)y * m->RowPitch + (size_t)x * 4;
    return (unsigned)p[2] << 16 | (unsigned)p[1] << 8 | p[0];
}

int main(void) {
    HMODULE d3d10_1 = LoadLibraryW(L"d3d10_1.dll");
    if (!d3d10_1) {
        printf("d3d10_1_test: skipped (no d3d10_1.dll)\n");
        return 0;
    }
    PFN_CREATE1 create = (PFN_CREATE1)(void *)GetProcAddress(d3d10_1, "D3D10CreateDevice1");
    if (!create) {
        printf("d3d10_1_test: skipped (no D3D10CreateDevice1)\n");
        return 0;
    }
    HMODULE d2d1 = LoadLibraryW(L"d2d1.dll");
    if (!d2d1) {
        printf("d3d10_1_test: skipped (no d2d1.dll)\n");
        return 0;
    }
    PFN_D2D1_CREATE_FACTORY d2d1_create =
        (PFN_D2D1_CREATE_FACTORY)(void *)GetProcAddress(d2d1, "D2D1CreateFactory");
    if (!d2d1_create) {
        printf("d3d10_1_test: skipped (no D2D1CreateFactory)\n");
        return 0;
    }

    /* The SDK version is checked before anything else: every other value
     * is E_INVALIDARG, with no device. */
    ID3D10Device1 *bad = (ID3D10Device1 *)(void *)&check;
    last = create(NULL, D3D10_DRIVER_TYPE_HARDWARE, NULL, 0, D3D10_FEATURE_LEVEL_10_1,
                  D3D10_1_SDK_VERSION + 1, &bad);
    CHECK(last == E_INVALIDARG);
    CHECK(bad == NULL);

    ID3D10Device1 *dev = NULL;
    last = create(NULL, D3D10_DRIVER_TYPE_HARDWARE, NULL, D3D10_CREATE_DEVICE_BGRA_SUPPORT,
                  D3D10_FEATURE_LEVEL_10_1, D3D10_1_SDK_VERSION, &dev);
    if (FAILED(last)) {
        printf("d3d10_1_test: skipped (no device, hr 0x%08lx)\n", (unsigned long)last);
        return 0;
    }
    CHECK(dev != NULL);
    CHECK(ID3D10Device1_GetFeatureLevel(dev) >= D3D10_FEATURE_LEVEL_10_1);

    /* The adapter behind the device names the driver. */
    IDXGIDevice *dxgi_dev = NULL;
    HR(qi(dev, &IID_IDXGIDevice, (void **)&dxgi_dev));
    IDXGIAdapter *adapter = NULL;
    HR(IDXGIDevice_GetAdapter(dxgi_dev, &adapter));
    DXGI_ADAPTER_DESC ad;
    HR(IDXGIAdapter_GetDesc(adapter, &ad));
    char name[128];
    size_t i;
    for (i = 0; i < sizeof name - 1 && ad.Description[i]; i++)
        name[i] = ad.Description[i] < 0x80 ? (char)ad.Description[i] : '?';
    name[i] = 0;
    printf("d3d10_1_test: device %s\n", name);
    fflush(stdout);
    rel(adapter);
    rel(dxgi_dev);

    /* The texture Direct2D will draw into, and the staging texture the
     * frame is read back through. */
    D3D10_TEXTURE2D_DESC td;
    memset(&td, 0, sizeof td);
    td.Width = SIDE;
    td.Height = SIDE;
    td.MipLevels = 1;
    td.ArraySize = 1;
    td.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    td.SampleDesc.Count = 1;
    td.Usage = D3D10_USAGE_DEFAULT;
    td.BindFlags = D3D10_BIND_RENDER_TARGET;
    ID3D10Texture2D *tex = NULL;
    HR(ID3D10Device1_CreateTexture2D(dev, &td, NULL, &tex));
    CHECK(tex != NULL);

    D3D10_TEXTURE2D_DESC got;
    memset(&got, 0, sizeof got);
    ID3D10Texture2D_GetDesc(tex, &got);
    CHECK(got.Width == SIDE && got.Height == SIDE);
    CHECK(got.Format == DXGI_FORMAT_B8G8R8A8_UNORM);
    CHECK(got.BindFlags == D3D10_BIND_RENDER_TARGET);
    CHECK(got.MipLevels == 1 && got.ArraySize == 1);

    /* A resource knows the device it came from. */
    ID3D10Device *back = NULL;
    ID3D10Texture2D_GetDevice(tex, &back);
    CHECK(back != NULL);
    rel(back);

    D3D10_TEXTURE2D_DESC sd = td;
    sd.Usage = D3D10_USAGE_STAGING;
    sd.BindFlags = 0;
    sd.CPUAccessFlags = D3D10_CPU_ACCESS_READ;
    ID3D10Texture2D *staging = NULL;
    HR(ID3D10Device1_CreateTexture2D(dev, &sd, NULL, &staging));
    CHECK(staging != NULL);

    /* The texture as a DXGI surface is what Direct2D draws onto. */
    IDXGISurface *surface = NULL;
    HR(qi(tex, &IID_IDXGISurface, (void **)&surface));
    CHECK(surface != NULL);

    ID2D1Factory *factory = NULL;
    HR(d2d1_create(D2D1_FACTORY_TYPE_SINGLE_THREADED, &IID_ID2D1Factory, NULL, (void **)&factory));
    CHECK(factory != NULL);

    D2D1_RENDER_TARGET_PROPERTIES props;
    memset(&props, 0, sizeof props);
    props.type = D2D1_RENDER_TARGET_TYPE_DEFAULT;
    props.pixelFormat.format = DXGI_FORMAT_B8G8R8A8_UNORM;
    props.pixelFormat.alphaMode = D2D1_ALPHA_MODE_PREMULTIPLIED;
    props.usage = D2D1_RENDER_TARGET_USAGE_NONE;
    props.minLevel = D2D1_FEATURE_LEVEL_DEFAULT;
    ID2D1RenderTarget *rt = NULL;
    HR(ID2D1Factory_CreateDxgiSurfaceRenderTarget(factory, surface, &props, &rt));
    CHECK(rt != NULL);

    D2D1_COLOR_F fill;
    fill.r = 1.0f;
    fill.g = 0.8f;
    fill.b = 0.0f;
    fill.a = 1.0f;
    ID2D1SolidColorBrush *brush = NULL;
    HR(ID2D1RenderTarget_CreateSolidColorBrush(rt, &fill, NULL, &brush));
    CHECK(brush != NULL);

    D2D1_COLOR_F clear;
    clear.r = 0.2f;
    clear.g = 0.4f;
    clear.b = 0.6f;
    clear.a = 1.0f;
    D2D1_RECT_F middle;
    middle.left = SIDE / 4.0f;
    middle.top = SIDE / 4.0f;
    middle.right = SIDE * 3 / 4.0f;
    middle.bottom = SIDE * 3 / 4.0f;
    ID2D1RenderTarget_BeginDraw(rt);
    ID2D1RenderTarget_Clear(rt, &clear);
    ID2D1RenderTarget_FillRectangle(rt, &middle, (ID2D1Brush *)brush);
    HR(ID2D1RenderTarget_EndDraw(rt, NULL, NULL));

    /* What Direct2D drew has to be in the Direct3D texture. */
    ID3D10Device1_CopyResource(dev, (ID3D10Resource *)staging, (ID3D10Resource *)tex);
    D3D10_MAPPED_TEXTURE2D mapped;
    memset(&mapped, 0, sizeof mapped);
    HR(ID3D10Texture2D_Map(staging, 0, D3D10_MAP_READ, 0, &mapped));
    unsigned corner = pixel(&mapped, 4, 4);
    unsigned centre = pixel(&mapped, SIDE / 2, SIDE / 2);
    ID3D10Texture2D_Unmap(staging, 0);
    printf("d3d10_1_test: read back 0x%06x 0x%06x\n", corner, centre);
    fflush(stdout);
    CHECK(corner == 0x336699);
    CHECK(centre == 0xffcc00);

    rel(brush);
    rel(rt);
    rel(factory);
    rel(surface);
    rel(staging);
    rel(tex);
    rel(dev);
    printf("d3d10_1_test: ok\n");
    fflush(stdout);
    return 0;
}
