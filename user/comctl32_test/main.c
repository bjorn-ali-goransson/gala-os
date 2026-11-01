/* comctl32 as real Windows behaves (documented behaviour only), for what
 * wxWidgets (FileZilla) uses, added as FileZilla's missing-features
 * ledger names it:
 *
 * - Image lists: ImageList_Create gives an empty list of the size asked
 *   (GetImageCount 0, GetIconSize). ImageList_Add splits a bitmap into
 *   images of the list's width and returns the first one's index;
 *   ImageList_AddMasked makes the mask from a colour (those pixels are
 *   transparent). ImageList_Draw: ILD_NORMAL fills the background colour
 *   (SetBkColor, CLR_NONE at first: none) under transparent pixels,
 *   ILD_TRANSPARENT leaves them. ImageList_GetImageInfo gives the
 *   image's bitmaps and a rectangle of the image's size.
 *   ImageList_ReplaceIcon with -1 appends an icon; ImageList_GetIcon
 *   makes one. ImageList_Copy with ILCF_SWAP swaps two images.
 *   ImageList_Replace replaces one. ImageList_Remove removes one (-1:
 *   all). ImageList_Destroy.
 *
 * Prints "comctl32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <commctrl.h>
#include <stdio.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("comctl32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

#define RED 0xff0000
#define GREEN 0x00ff00
#define BLUE 0x0000ff
#define MAGENTA 0xff00ff

/* A w x h 32-bit DIB section whose columns [0, split) are `left` and the
 * rest `right` (0x00RRGGBB). */
static HBITMAP dib(int w, int h, int split, DWORD left, DWORD right) {
    BITMAPINFO bi = {{sizeof(BITMAPINFOHEADER), w, -h, 1, 32, BI_RGB}};
    DWORD *bits;
    HBITMAP b = CreateDIBSection(NULL, &bi, DIB_RGB_COLORS, (void **)&bits, NULL, 0);
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) bits[y * w + x] = x < split ? left : right;
    return b;
}

static COLORREF rgb(DWORD c) { return RGB(c >> 16, (c >> 8) & 0xff, c & 0xff); }

int main(void) {
    HDC screen = GetDC(NULL);
    HDC dc = CreateCompatibleDC(screen);
    HBITMAP canvas = CreateCompatibleBitmap(screen, 64, 64);
    HGDIOBJ old = SelectObject(dc, canvas);

    HIMAGELIST il = ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 4, 4);
    CHECK(il != NULL && ImageList_GetImageCount(il) == 0);
    int cx = 0, cy = 0;
    CHECK(ImageList_GetIconSize(il, &cx, &cy) && cx == 16 && cy == 16);

    // Two images from one 32 x 16 bitmap: red, then blue.
    HBITMAP two = dib(32, 16, 16, RED, BLUE);
    CHECK(ImageList_Add(il, two, NULL) == 0 && ImageList_GetImageCount(il) == 2);
    // A green image whose left half is magenta, the mask colour.
    HBITMAP keyed = dib(16, 16, 8, MAGENTA, GREEN);
    CHECK(ImageList_AddMasked(il, keyed, rgb(MAGENTA)) == 2 && ImageList_GetImageCount(il) == 3);

    PatBlt(dc, 0, 0, 64, 64, WHITENESS);
    CHECK(ImageList_Draw(il, 0, dc, 0, 0, ILD_NORMAL));
    CHECK(GetPixel(dc, 0, 0) == RGB(255, 0, 0) && GetPixel(dc, 15, 15) == RGB(255, 0, 0));
    CHECK(GetPixel(dc, 16, 0) == RGB(255, 255, 255));
    CHECK(ImageList_Draw(il, 1, dc, 20, 0, ILD_TRANSPARENT) && GetPixel(dc, 20, 0) == RGB(0, 0, 255));
    // Transparent where the mask colour was.
    CHECK(ImageList_Draw(il, 2, dc, 40, 0, ILD_TRANSPARENT));
    CHECK(GetPixel(dc, 40, 0) == RGB(255, 255, 255) && GetPixel(dc, 48, 0) == RGB(0, 255, 0));
    // The background colour: CLR_NONE at first; ILD_NORMAL fills it.
    CHECK(ImageList_SetBkColor(il, RGB(0, 0, 0)) == CLR_NONE && ImageList_GetBkColor(il) == RGB(0, 0, 0));
    CHECK(ImageList_Draw(il, 2, dc, 40, 20, ILD_NORMAL));
    CHECK(GetPixel(dc, 40, 20) == RGB(0, 0, 0) && GetPixel(dc, 48, 20) == RGB(0, 255, 0));
    ImageList_SetBkColor(il, CLR_NONE);

    IMAGEINFO ii;
    CHECK(ImageList_GetImageInfo(il, 1, &ii) && ii.hbmImage != NULL);
    CHECK(ii.rcImage.right - ii.rcImage.left == 16 && ii.rcImage.bottom - ii.rcImage.top == 16);

    // An icon appended, and one made from an image.
    HBITMAP icolor = dib(16, 16, 16, GREEN, GREEN);
    HBITMAP imask = CreateBitmap(16, 16, 1, 1, NULL);
    ICONINFO info = {TRUE, 0, 0, imask, icolor};
    HICON icon = CreateIconIndirect(&info);
    CHECK(icon != NULL && ImageList_ReplaceIcon(il, -1, icon) == 3 && ImageList_GetImageCount(il) == 4);
    CHECK(ImageList_Draw(il, 3, dc, 0, 40, ILD_NORMAL) && GetPixel(dc, 5, 45) == RGB(0, 255, 0));
    HICON made = ImageList_GetIcon(il, 0, ILD_NORMAL);
    ICONINFO back;
    CHECK(made != NULL && GetIconInfo(made, &back) && back.hbmColor != NULL);
    DeleteObject(back.hbmColor);
    DeleteObject(back.hbmMask);
    DestroyIcon(made);
    DestroyIcon(icon);

    // Swap 0 and 1: image 0 is blue now.
    CHECK(ImageList_Copy(il, 0, il, 1, ILCF_SWAP));
    CHECK(ImageList_Draw(il, 0, dc, 0, 0, ILD_NORMAL) && GetPixel(dc, 0, 0) == RGB(0, 0, 255));
    // Replace image 1 (red now) with green.
    CHECK(ImageList_Replace(il, 1, icolor, NULL));
    CHECK(ImageList_Draw(il, 1, dc, 0, 0, ILD_NORMAL) && GetPixel(dc, 0, 0) == RGB(0, 255, 0));
    // Remove the first: the green one is first.
    CHECK(ImageList_Remove(il, 0) && ImageList_GetImageCount(il) == 3);
    CHECK(ImageList_Draw(il, 0, dc, 0, 0, ILD_NORMAL) && GetPixel(dc, 0, 0) == RGB(0, 255, 0));
    CHECK(ImageList_Remove(il, -1) && ImageList_GetImageCount(il) == 0);
    CHECK(ImageList_Destroy(il));

    DeleteObject(two);
    DeleteObject(keyed);
    DeleteObject(icolor);
    DeleteObject(imask);
    SelectObject(dc, old);
    DeleteObject(canvas);
    DeleteDC(dc);
    ReleaseDC(NULL, screen);
    printf("comctl32_test: ok\n");
    return 0;
}
