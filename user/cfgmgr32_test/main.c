/* cfgmgr32's device-tree queries as the Khronos Vulkan loader makes them
 * (it lists the display adapters' devices for their Vulkan drivers before
 * reading HKLM\SOFTWARE\Khronos\Vulkan\Drivers). Expected values are from
 * Microsoft's documentation (CM_Get_Device_ID_List_SizeW,
 * CM_Get_Device_ID_ListW, CM_Locate_DevNodeW, CM_Get_DevNode_Status,
 * CM_Get_Child, CM_Get_Device_IDW) and hold on any Windows machine:
 *
 * - A class no device has (a made-up GUID, CM_GETIDLIST_FILTER_CLASS |
 *   CM_GETIDLIST_FILTER_PRESENT): the size is at least 1 (the list's
 *   final NUL) and the list is empty; a NULL length is
 *   CR_INVALID_POINTER.
 * - CM_Locate_DevNodeW of an instance ID no device has:
 *   CR_NO_SUCH_DEVNODE. With NULL it is the root of the device tree,
 *   which is started (DN_STARTED), and whose ID is HTREE\ROOT\0.
 *
 * Prints "cfgmgr32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <cfgmgr32.h>
#include <stdio.h>
#include <wchar.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("cfgmgr32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

int main(void) {
    const WCHAR *none = L"{6a1d6e2c-9b0e-4f55-8a6e-2f0b8a1e7c42}";
    ULONG flags = CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT;
    ULONG len = 0;
    CHECK(CM_Get_Device_ID_List_SizeW(&len, none, flags) == CR_SUCCESS);
    CHECK(len >= 1 && len < 4096);
    CHECK(CM_Get_Device_ID_List_SizeW(NULL, none, flags) == CR_INVALID_POINTER);
    WCHAR list[4096];
    list[0] = L'x';
    CHECK(CM_Get_Device_ID_ListW(none, list, len, flags) == CR_SUCCESS);
    CHECK(list[0] == 0);

    DEVINST dev = 0;
    CHECK(CM_Locate_DevNodeW(&dev, (DEVINSTID_W)L"ROOT\\GALA_NO_SUCH_DEVICE\\0000", CM_LOCATE_DEVNODE_NORMAL) ==
          CR_NO_SUCH_DEVNODE);
    CHECK(CM_Locate_DevNodeW(&dev, NULL, CM_LOCATE_DEVNODE_NORMAL) == CR_SUCCESS && dev != 0);
    ULONG status = 0, problem = 0;
    CHECK(CM_Get_DevNode_Status(&status, &problem, dev, 0) == CR_SUCCESS);
    CHECK(status & DN_STARTED);
    WCHAR id[64];
    CHECK(CM_Get_Device_IDW(dev, id, 64, 0) == CR_SUCCESS);
    CHECK(wcscmp(id, L"HTREE\\ROOT\\0") == 0);
    printf("cfgmgr32_test: ok\n");
    return 0;
}
