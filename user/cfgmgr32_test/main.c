/* cfgmgr32's device-tree queries: the device lists a program asks the
 * configuration manager for, and the devnode each id in them names. The
 * Khronos Vulkan loader asks for the display adapters' devices before it
 * reads HKLM\SOFTWARE\Khronos\Vulkan\Drivers, and Word asks for the
 * present devices of a class sixteen times before it draws anything.
 *
 * Expected values are Microsoft's documentation's
 * (CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW,
 * CM_Locate_DevNodeW, CM_Get_DevNode_Status, CM_Get_Device_IDW,
 * CM_Get_DevNode_Registry_PropertyW) and, where it does not say, this
 * machine's measured ones. They hold on any Windows machine: the lists
 * are asked about themselves and about each other rather than for a
 * device some machine may not have.
 *
 * - A class no device has (a made-up GUID, CM_GETIDLIST_FILTER_CLASS |
 *   CM_GETIDLIST_FILTER_PRESENT): the size is at least 1 (the list's
 *   final NUL) and the list is empty; a NULL length is
 *   CR_INVALID_POINTER. A class filter with no string is
 *   CR_INVALID_POINTER, one whose string is not a GUID is
 *   CR_INVALID_DATA, and a flag that is no filter is CR_INVALID_FLAG.
 * - Unfiltered, the list holds at least one device, and every id in it
 *   is <enumerator>\<hardware id>\<instance>.
 * - CM_GETIDLIST_FILTER_PRESENT holds at least one device, no more than
 *   the unfiltered list, and every id of it is in that list too.
 * - A buffer too small for a list is CR_BUFFER_SMALL, and nothing is
 *   written past its end.
 * - The first present id names a devnode, which gives that same id
 *   back, has a status, and has a class GUID (REG_SZ, 38 characters);
 *   the list of that class holds that device and is no longer than the
 *   present list. So does the list of its enumerator, and of its
 *   service where it has one.
 * - A devnode handle no device has is CR_INVALID_DEVNODE.
 * - CM_Locate_DevNodeW of an instance id no device has is
 *   CR_NO_SUCH_DEVNODE. With NULL it is the root of the device tree,
 *   which is started (DN_STARTED), whose id is HTREE\ROOT\0, and whose
 *   handle is not any device's -- the configuration manager's devnodes
 *   and setupapi's SP_DEVINFO_DATA.DevInst are one numbering.
 *
 * Prints what it measured, then "cfgmgr32_test: ok", or exits with the
 * number of the failing check. */
#include <windows.h>
#include <cfgmgr32.h>
#include <stdio.h>
#include <wchar.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("cfgmgr32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

/* Room for a whole list: this machine's 218 devices take about 9600
 * units, and a list that does not fit is a failed check and not a
 * buffer run past. */
#define LIST_WIDE 32768

static WCHAR all_list[LIST_WIDE];
static WCHAR present_list[LIST_WIDE];
static WCHAR class_list[LIST_WIDE];

/* ASCII case folding, which is all the comparisons below need: every
 * name in the tree is a hardware id or a GUID. */
static WCHAR fold(WCHAR c) { return (c >= L'A' && c <= L'Z') ? (WCHAR)(c - L'A' + L'a') : c; }

static int same_id(const WCHAR *a, const WCHAR *b) {
    while (*a && fold(*a) == fold(*b)) { a++; b++; }
    return *a == 0 && *b == 0;
}

/* Whether a multi-sz list holds this id. */
static int holds(const WCHAR *list, const WCHAR *id) {
    const WCHAR *at;
    for (at = list; *at; at += wcslen(at) + 1) {
        if (same_id(at, id)) { return 1; }
    }
    return 0;
}

/* How many ids a multi-sz list holds, or -1 if one of them is not
 * <enumerator>\<hardware id>\<instance>. */
static long ids(const WCHAR *list) {
    const WCHAR *at;
    long n = 0;
    for (at = list; *at; at += wcslen(at) + 1) {
        const WCHAR *c;
        int slashes = 0;
        for (c = at; *c; c++) { if (*c == L'\\') { slashes++; } }
        if (slashes < 2) { return -1; }
        n++;
    }
    return n;
}

/* One list, fetched the way a program does: its size, then its text.
 * A size that is stale by the time the list is asked for is the
 * documented CR_BUFFER_SMALL, so the pair is asked again -- the tree
 * can gain a device between the two calls on a real machine. */
static CONFIGRET fetch(WCHAR *into, const WCHAR *filter, ULONG flags, ULONG *len) {
    int tries;
    for (tries = 0; tries < 3; tries++) {
        CONFIGRET r = CM_Get_Device_ID_List_SizeW(len, filter, flags);
        if (r != CR_SUCCESS) { return r; }
        if (*len > LIST_WIDE) { return CR_BUFFER_SMALL; }
        r = CM_Get_Device_ID_ListW(filter, into, *len, flags);
        if (r != CR_BUFFER_SMALL) { return r; }
    }
    return CR_BUFFER_SMALL;
}

/* A wide name as ASCII, for printing it without a wide format. */
static void narrow(const WCHAR *from, char *into, int room) {
    int i;
    for (i = 0; i + 1 < room && from[i]; i++) {
        into[i] = (from[i] < 0x80) ? (char)from[i] : '?';
    }
    into[i] = 0;
}

int main(void) {
    const WCHAR *none = L"{6a1d6e2c-9b0e-4f55-8a6e-2f0b8a1e7c42}";
    ULONG flags = CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT;
    ULONG len = 0, all_len = 0, present_len = 0, class_len = 0;
    long all_n, present_n, class_n;
    const WCHAR *at;
    WCHAR empty[4096], small[2], back[MAX_DEVICE_ID_LEN + 1];
    WCHAR guid[64], service[256], enumerator[MAX_DEVICE_ID_LEN + 1], id[64];
    char text[128];
    DEVINST dev = 0, root = 0;
    ULONG status = 0, problem = 0, guid_len, service_len, kind = 0;
    int i;

    /* A class no device has: the size of an empty list, and the empty
     * list itself. */
    CHECK(CM_Get_Device_ID_List_SizeW(&len, none, flags) == CR_SUCCESS);
    CHECK(len >= 1 && len < 4096);
    CHECK(CM_Get_Device_ID_List_SizeW(NULL, none, flags) == CR_INVALID_POINTER);
    empty[0] = L'x';
    CHECK(CM_Get_Device_ID_ListW(none, empty, len, flags) == CR_SUCCESS);
    CHECK(empty[0] == 0);

    /* A class filter needs a class, and a flag that is no filter is
     * refused outright. */
    CHECK(CM_Get_Device_ID_List_SizeW(&len, NULL, flags) == CR_INVALID_POINTER);
    CHECK(CM_Get_Device_ID_List_SizeW(&len, L"not-a-guid", flags) == CR_INVALID_DATA);
    CHECK(CM_Get_Device_ID_List_SizeW(&len, NULL, 0x40000000) == CR_INVALID_FLAG);

    /* Every device the tree holds, and every id of it a three-part
     * instance id. */
    CHECK(fetch(all_list, NULL, CM_GETIDLIST_FILTER_NONE, &all_len) == CR_SUCCESS);
    all_n = ids(all_list);
    printf("cfgmgr32_test: %ld devices, %lu units\n", all_n, (unsigned long)all_len);
    CHECK(all_n >= 1);

    /* The ones that are there now: a part of the whole, and no more. */
    CHECK(fetch(present_list, NULL, CM_GETIDLIST_FILTER_PRESENT, &present_len) == CR_SUCCESS);
    present_n = ids(present_list);
    printf("cfgmgr32_test: %ld present, %lu units\n", present_n, (unsigned long)present_len);
    CHECK(present_n >= 1 && present_n <= all_n);
    for (at = present_list; *at; at += wcslen(at) + 1) { CHECK(holds(all_list, at)); }

    /* A buffer that cannot hold the list is refused, and nothing is
     * written past its end. */
    small[0] = L'x';
    small[1] = L'y';
    CHECK(CM_Get_Device_ID_ListW(NULL, small, 1, CM_GETIDLIST_FILTER_PRESENT) == CR_BUFFER_SMALL);
    CHECK(small[1] == L'y');

    /* The first present id names a devnode, and that devnode gives the
     * same id back. */
    narrow(present_list, text, sizeof text);
    printf("cfgmgr32_test: first present %s\n", text);
    CHECK(CM_Locate_DevNodeW(&dev, (DEVINSTID_W)present_list, CM_LOCATE_DEVNODE_NORMAL) == CR_SUCCESS);
    CHECK(dev != 0);
    CHECK(CM_Get_Device_IDW(dev, back, MAX_DEVICE_ID_LEN + 1, 0) == CR_SUCCESS);
    CHECK(same_id(back, present_list));
    CHECK(CM_Get_Device_IDW(dev, back, 1, 0) == CR_BUFFER_SMALL);
    CHECK(CM_Get_DevNode_Status(&status, &problem, dev, 0) == CR_SUCCESS);

    /* Its class, and the present devices of that class: they hold it,
     * and are no more than the present devices. */
    guid_len = sizeof guid;
    CHECK(CM_Get_DevNode_Registry_PropertyW(dev, CM_DRP_CLASSGUID, &kind, guid, &guid_len, 0) == CR_SUCCESS);
    CHECK(kind == REG_SZ);
    CHECK(wcslen(guid) == 38 && guid[0] == L'{' && guid[37] == L'}');
    narrow(guid, text, sizeof text);
    printf("cfgmgr32_test: its class %s\n", text);
    CHECK(fetch(class_list, guid, flags, &class_len) == CR_SUCCESS);
    class_n = ids(class_list);
    printf("cfgmgr32_test: %ld of that class present\n", class_n);
    CHECK(class_n >= 1 && class_n <= present_n);
    CHECK(holds(class_list, present_list));

    /* The devices of its enumerator -- the first part of its id -- hold
     * it too. */
    for (i = 0; present_list[i] && present_list[i] != L'\\' && i < MAX_DEVICE_ID_LEN; i++) {
        enumerator[i] = present_list[i];
    }
    enumerator[i] = 0;
    CHECK(i > 0);
    CHECK(fetch(class_list, enumerator, CM_GETIDLIST_FILTER_ENUMERATOR, &class_len) == CR_SUCCESS);
    narrow(enumerator, text, sizeof text);
    printf("cfgmgr32_test: %ld under %s\n", ids(class_list), text);
    CHECK(ids(class_list) >= 1);
    CHECK(holds(class_list, present_list));

    /* And the devices of its service, where its key names one. */
    service_len = sizeof service;
    kind = 0;
    if (CM_Get_DevNode_Registry_PropertyW(dev, CM_DRP_SERVICE, &kind, service, &service_len, 0) == CR_SUCCESS) {
        CHECK(kind == REG_SZ);
        CHECK(fetch(class_list, service, CM_GETIDLIST_FILTER_SERVICE, &class_len) == CR_SUCCESS);
        narrow(service, text, sizeof text);
        printf("cfgmgr32_test: %ld driven by %s\n", ids(class_list), text);
        CHECK(ids(class_list) >= 1);
        CHECK(holds(class_list, present_list));
    }

    /* A devnode handle no device has. */
    CHECK(CM_Get_DevNode_Status(&status, &problem, 0x7fffffff, 0) == CR_INVALID_DEVNODE);
    CHECK(CM_Get_Device_IDW(0x7fffffff, id, 64, 0) == CR_INVALID_DEVNODE);

    /* An id no device has, and the root of the tree -- whose handle is
     * no device's, the two being one numbering. */
    CHECK(CM_Locate_DevNodeW(&root, (DEVINSTID_W)L"ROOT\\GALA_NO_SUCH_DEVICE\\0000", CM_LOCATE_DEVNODE_NORMAL) ==
          CR_NO_SUCH_DEVNODE);
    CHECK(CM_Locate_DevNodeW(&root, NULL, CM_LOCATE_DEVNODE_NORMAL) == CR_SUCCESS && root != 0);
    CHECK(root != dev);
    status = 0;
    problem = 0;
    CHECK(CM_Get_DevNode_Status(&status, &problem, root, 0) == CR_SUCCESS);
    CHECK(status & DN_STARTED);
    CHECK(CM_Get_Device_IDW(root, id, 64, 0) == CR_SUCCESS);
    CHECK(wcscmp(id, L"HTREE\\ROOT\\0") == 0);
    printf("cfgmgr32_test: ok\n");
    return 0;
}
