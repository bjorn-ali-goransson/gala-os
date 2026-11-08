/* OpenGL through WGL, as a Windows program draws with it (ROADMAP GPU G2):
 * a window, a pixel format, a context, a cleared frame with a triangle,
 * read back and presented.
 *
 * Expected values are what Microsoft's documentation gives for gdi32's
 * pixel-format functions and opengl32's WGL, and what Wine's
 * dlls/opengl32/tests (opengl.c: test_setpixelformat) check on Windows:
 *
 * - ChoosePixelFormat for a double-buffered RGBA window format with
 *   OpenGL gives a format (1-based); DescribePixelFormat fills the
 *   descriptor and returns the number of formats (at least that one),
 *   with PFD_DRAW_TO_WINDOW, PFD_SUPPORT_OPENGL and PFD_DOUBLEBUFFER and
 *   at least 24 colour bits.
 * - A new window's DC has no format (GetPixelFormat 0); SetPixelFormat
 *   sets it once: GetPixelFormat then returns it, setting the same one
 *   again succeeds and a different one fails.
 * - wglCreateContext and wglMakeCurrent succeed; wglGetCurrentContext and
 *   wglGetCurrentDC return them. glGetString(GL_VENDOR, GL_RENDERER,
 *   GL_VERSION) are strings (printed: on gala Mesa's llvmpipe, which the
 *   scenario expects; Windows without a GPU driver has its own software
 *   renderer, which draws the same pixels).
 * - glClear to (0.2, 0.4, 0.6) and a flat (1.0, 0.8, 0.0) triangle over
 *   most of the client area: glReadPixels of the back buffer gives
 *   0x336699 in a corner and 0xffcc00 inside the triangle; glGetError is
 *   GL_NO_ERROR. SwapBuffers succeeds and the window shows the frame
 *   (xtask's screen check).
 * - A click in the window ends the program (it waits up to 60 s for it:
 *   without one, a failure); the context is released and deleted.
 *
 * Prints "gl_test: presented" once the frame is on the screen (xtask then
 * checks it and clicks), then "gl_test: ok", or exits with the number of
 * the failing check. */
#include <windows.h>
#include <GL/gl.h>
#include <stdio.h>
#include <string.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("gl_test: check %d (line %d) failed (error %lu, gl 0x%x)\n", check, __LINE__, GetLastError(), (unsigned)glGetError()); fflush(stdout); ExitProcess(check); } } while (0)

static volatile LONG clicked;

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_LBUTTONDOWN:
        clicked = 1;
        PostQuitMessage(0);
        return 0;
    case WM_TIMER:
        /* No click within 60 s. */
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

/* The back buffer's pixel at client (x, y) (y from the top) as 0xRRGGBB. */
static unsigned pixel(int x, int y, int height) {
    unsigned char rgba[4] = {0};
    glReadPixels(x, height - 1 - y, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, rgba);
    return (unsigned)rgba[0] << 16 | (unsigned)rgba[1] << 8 | rgba[2];
}

int main(void) {
    HINSTANCE inst = GetModuleHandleW(NULL);
    WNDCLASSW wc;
    memset(&wc, 0, sizeof wc);
    wc.style = CS_OWNDC;
    wc.lpfnWndProc = proc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursorW(NULL, (LPCWSTR)IDC_ARROW);
    wc.lpszClassName = L"gl_test";
    CHECK(RegisterClassW(&wc) != 0);
    HWND hwnd = CreateWindowExW(0, L"gl_test", L"OpenGL", WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                                100, 100, 400, 300, NULL, NULL, inst, NULL);
    CHECK(hwnd != NULL);
    RECT client;
    CHECK(GetClientRect(hwnd, &client));
    int w = client.right, h = client.bottom;
    CHECK(w > 100 && h > 100);
    printf("gl_test: client %dx%d\n", w, h);

    HDC hdc = GetDC(hwnd);
    CHECK(hdc != NULL);
    CHECK(GetPixelFormat(hdc) == 0);

    PIXELFORMATDESCRIPTOR want;
    memset(&want, 0, sizeof want);
    want.nSize = sizeof want;
    want.nVersion = 1;
    want.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
    want.iPixelType = PFD_TYPE_RGBA;
    want.cColorBits = 32;
    want.cDepthBits = 24;
    want.iLayerType = PFD_MAIN_PLANE;
    int format = ChoosePixelFormat(hdc, &want);
    CHECK(format > 0);

    PIXELFORMATDESCRIPTOR got;
    memset(&got, 0, sizeof got);
    int count = DescribePixelFormat(hdc, format, sizeof got, &got);
    CHECK(count >= format);
    DWORD need = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
    CHECK((got.dwFlags & need) == need);
    CHECK(got.iPixelType == PFD_TYPE_RGBA && got.cColorBits >= 24);
    printf("gl_test: format %d of %d\n", format, count);

    CHECK(SetPixelFormat(hdc, format, &got));
    CHECK(GetPixelFormat(hdc) == format);
    CHECK(SetPixelFormat(hdc, format, &got));
    /* Another format: one that differs in its flags (or depth). */
    int other = 0;
    for (int i = 1; i <= count && !other; i++) {
        PIXELFORMATDESCRIPTOR p;
        if (i != format && DescribePixelFormat(hdc, i, sizeof p, &p) &&
            (p.dwFlags != got.dwFlags || p.cDepthBits != got.cDepthBits))
            other = i;
    }
    if (other) {
        CHECK(!SetPixelFormat(hdc, other, &got));
        CHECK(GetPixelFormat(hdc) == format);
    }

    HGLRC ctx = wglCreateContext(hdc);
    CHECK(ctx != NULL);
    CHECK(wglMakeCurrent(hdc, ctx));
    CHECK(wglGetCurrentContext() == ctx);
    CHECK(wglGetCurrentDC() == hdc);

    const char *vendor = (const char *)glGetString(GL_VENDOR);
    const char *renderer = (const char *)glGetString(GL_RENDERER);
    const char *version = (const char *)glGetString(GL_VERSION);
    CHECK(vendor != NULL && renderer != NULL && version != NULL);
    printf("gl_test: vendor %s\n", vendor);
    printf("gl_test: renderer %s\n", renderer);
    printf("gl_test: version %s\n", version);
    fflush(stdout);

    glViewport(0, 0, w, h);
    glDisable(GL_DEPTH_TEST);
    glClearColor(0.2f, 0.4f, 0.6f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
    glShadeModel(GL_FLAT);
    glBegin(GL_TRIANGLES);
    glColor3f(1.0f, 0.8f, 0.0f);
    glVertex2f(-0.8f, -0.8f);
    glVertex2f(0.8f, -0.8f);
    glVertex2f(0.0f, 0.8f);
    glEnd();
    glFinish();
    CHECK(glGetError() == GL_NO_ERROR);

    /* The corner is the clear colour; (0, -0.3) in normalized device
     * coordinates is well inside the triangle. */
    glReadBuffer(GL_BACK);
    unsigned corner = pixel(4, 4, h), inside = pixel(w / 2, (int)(h * 0.65), h);
    printf("gl_test: read back 0x%06x 0x%06x\n", corner, inside);
    CHECK(corner == 0x336699);
    CHECK(inside == 0xffcc00);
    CHECK(glGetError() == GL_NO_ERROR);

    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    CHECK(SwapBuffers(hdc));
    printf("gl_test: presented\n");
    fflush(stdout);

    SetTimer(hwnd, 1, 60000, NULL);
    MSG msg;
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    CHECK(clicked);

    CHECK(wglMakeCurrent(NULL, NULL));
    CHECK(wglGetCurrentContext() == NULL);
    CHECK(wglDeleteContext(ctx));
    ReleaseDC(hwnd, hdc);
    DestroyWindow(hwnd);
    printf("gl_test: ok\n");
    return 0;
}
