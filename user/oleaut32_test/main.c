/* oleaut32 as real Windows behaves, for what programs import and gala
 * lacked (docs/ROADMAP.md AG): FileZilla Server's GUI and pgAdmin's
 * StackBuilder (wxWidgets 3.2) import SystemTimeToVariantTime,
 * VariantTimeToSystemTime, VarBstrFromCy and VarCyFromR8 (and
 * GetActiveObject); SQL Server LocalDB imports VarI4FromStr,
 * VarUI4FromStr, VarR8FromStr, VarBoolFromStr, VarCyFromStr,
 * VarDateFromStr, VarBstrFromBool and VarCmp.
 *
 * Expected values are from Microsoft's documentation (learn.microsoft.com:
 * "DATE type", SystemTimeToVariantTime, VariantTimeToSystemTime, VarCmp)
 * and from Wine's conformance tests (dlls/oleaut32/tests/vartest.c and
 * vartype.c), which are validated on real Windows; no Windows machine
 * was available to run this one, so it still needs a run-all.ps1 check
 * there. In short:
 *
 * - DATE: days since 30 December 1899 (0.0), the fraction the time of
 *   day; 2.0 is 1 January 1900 and -1.25 is 29 December 1899 06:00 (the
 *   fraction's magnitude is the time, also before 1899). Years 100-9999.
 * - SystemTimeToVariantTime: a day of 0 is the last day of the month
 *   before; a month over 12 or a day over 31 fails; years 0-49 mean
 *   2000-2049 and 50-99 1950-1999 (Windows 10 1903 and later).
 * - VariantTimeToSystemTime: to the second (2958465.999999 is
 *   31 December 9999 23:59:59, milliseconds 0), wDayOfWeek set.
 * - VarBstrFromCy: the currency's value with up to four decimals and no
 *   trailing zeros. VarCyFromR8: times 10000, half to even.
 * - VarXxxFromStr: numbers in the locale's format (en-US here), rounded
 *   half to even for integers; DISP_E_OVERFLOW out of range,
 *   DISP_E_TYPEMISMATCH for no number. VarBoolFromStr: "#TRUE#" and
 *   "#FALSE#" (case-sensitive), English true/false in any case, or a
 *   number (non-zero is VARIANT_TRUE). VarBstrFromBool: "True"/"False".
 * - VarDateFromStr: en-US dates (month/day/year, or day/month when the
 *   first number cannot be a month; year-month-day; English month
 *   names) and times (h:mm[:ss], AM/PM; 13:00 PM is 13:00).
 * - VarCmp: VARCMP_LT/EQ/GT/NULL; numbers by value, strings as
 *   strings, a string is greater than any number, an empty string
 *   equals VT_EMPTY.
 * - GetActiveObject: MK_E_UNAVAILABLE for a class nothing registered as
 *   running.
 *
 * Prints "oleaut32_test: ok" or exits with the number of the failing
 * check. */
#include <windows.h>
#include <oleauto.h>
#include <stdio.h>
#include <math.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("oleaut32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

#define EN_US MAKELCID(MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US), SORT_DEFAULT)

static int st2dt(WORD d, WORD m, WORD y, WORD h, WORD mn, WORD s, double *out) {
    SYSTEMTIME st = {0};
    st.wYear = y; st.wMonth = m; st.wDay = d; st.wHour = h; st.wMinute = mn; st.wSecond = s;
    *out = -12345.0;
    return SystemTimeToVariantTime(&st, out);
}

static int dt2st(double dt, WORD d, WORD m, WORD y, WORD h, WORD mn, WORD s) {
    SYSTEMTIME st;
    memset(&st, 0xcc, sizeof st);
    if (!VariantTimeToSystemTime(dt, &st))
        return 0;
    return st.wYear == y && st.wMonth == m && st.wDay == d && st.wHour == h && st.wMinute == mn
        && st.wSecond == s && st.wMilliseconds == 0;
}

static int bstr_is(BSTR b, const wchar_t *want) {
    return b != NULL && SysStringLen(b) == wcslen(want) && wcscmp(b, want) == 0;
}

static int cy_text(LONGLONG v, const wchar_t *want) {
    CY cy;
    cy.int64 = v;
    BSTR b = NULL;
    HRESULT hr = VarBstrFromCy(cy, EN_US, 0, &b);
    int ok = hr == S_OK && bstr_is(b, want);
    SysFreeString(b);
    return ok;
}

static HRESULT cy_from(double v, LONGLONG *out) {
    CY cy;
    cy.int64 = 0x5555;
    HRESULT hr = VarCyFromR8(v, &cy);
    *out = cy.int64;
    return hr;
}

static HRESULT i4(const wchar_t *s, LONG *out) {
    *out = 0x5555;
    return VarI4FromStr((OLECHAR *)s, EN_US, 0, out);
}

static HRESULT ui4(const wchar_t *s, ULONG *out) {
    *out = 0x5555;
    return VarUI4FromStr((OLECHAR *)s, EN_US, 0, out);
}

static HRESULT to_bool(const wchar_t *s, VARIANT_BOOL *out) {
    *out = 0x5555;
    return VarBoolFromStr((OLECHAR *)s, EN_US, 0, out);
}

static HRESULT date(const wchar_t *s, double *out) {
    *out = -12345.0;
    return VarDateFromStr((OLECHAR *)s, EN_US, 0, out);
}

static int close_to(double a, double b) { return fabs(a - b) < 1e-9; }

static HRESULT cmp(VARIANT *a, VARIANT *b) { return VarCmp(a, b, EN_US, 0); }

int main(void) {
    double dt;

    /* SystemTimeToVariantTime (vartest.c test_SystemTimeToVariantTime). */
    CHECK(st2dt(1, 1, 1980, 0, 0, 0, &dt) && dt == 29221.0);
    CHECK(st2dt(2, 1, 1980, 0, 0, 0, &dt) && dt == 29222.0);
    CHECK(st2dt(0, 1, 1980, 0, 0, 0, &dt) && dt == 29220.0);
    CHECK(!st2dt(1, 13, 1980, 0, 0, 0, &dt));
    CHECK(!st2dt(32, 1, 1980, 0, 0, 0, &dt));
    CHECK(!st2dt(1, 1, 10000, 0, 0, 0, &dt));
    CHECK(st2dt(1, 1, 9999, 0, 0, 0, &dt) && dt == 2958101.0);
    CHECK(st2dt(1, 1, 0, 0, 0, 0, &dt) && dt == 36526.0);
    CHECK(st2dt(1, 1, 29, 0, 0, 0, &dt) && dt == 47119.0);
    CHECK(st2dt(1, 1, 49, 0, 0, 0, &dt) && dt == 54424.0);
    CHECK(st2dt(1, 1, 50, 0, 0, 0, &dt) && dt == 18264.0);
    CHECK(st2dt(31, 12, 99, 0, 0, 0, &dt) && dt == 36525.0);
    /* The DATE type's documented examples, and a time of day. */
    CHECK(st2dt(30, 12, 1899, 0, 0, 0, &dt) && dt == 0.0);
    CHECK(st2dt(1, 1, 1900, 0, 0, 0, &dt) && dt == 2.0);
    CHECK(st2dt(29, 12, 1899, 6, 0, 0, &dt) && dt == -1.25);
    CHECK(st2dt(1, 1, 1980, 18, 0, 0, &dt) && dt == 29221.75);
    CHECK(st2dt(1, 1, 1980, 12, 0, 30, &dt) && close_to(dt, 29221.5 + 30.0 / 86400));

    /* VariantTimeToSystemTime. */
    CHECK(dt2st(29221.0, 1, 1, 1980, 0, 0, 0));
    CHECK(dt2st(29222.0, 2, 1, 1980, 0, 0, 0));
    CHECK(dt2st(2958465.999999, 31, 12, 9999, 23, 59, 59));
    CHECK(dt2st(0.0, 30, 12, 1899, 0, 0, 0));
    CHECK(dt2st(2.0, 1, 1, 1900, 0, 0, 0));
    CHECK(dt2st(-1.25, 29, 12, 1899, 6, 0, 0));
    CHECK(dt2st(29221.75, 1, 1, 1980, 18, 0, 0));
    CHECK(dt2st(29221.5 + 30.0 / 86400, 1, 1, 1980, 12, 0, 30));
    CHECK(dt2st(36585.0, 29, 2, 2000, 0, 0, 0)); /* a leap day */
    SYSTEMTIME st;
    CHECK(VariantTimeToSystemTime(29221.0, &st) && st.wDayOfWeek == 2); /* a Tuesday */
    CHECK(!VariantTimeToSystemTime(2958466.0, &st)); /* 1 January 10000 */
    /* A round trip through both. */
    CHECK(st2dt(15, 6, 2024, 13, 45, 7, &dt) && dt2st(dt, 15, 6, 2024, 13, 45, 7));

    /* VarBstrFromCy (vartype.c test_VarBstrFromCy). */
    CHECK(cy_text(0, L"0"));
    CHECK(cy_text(10000, L"1"));
    CHECK(cy_text(15000, L"1.5"));
    CHECK(cy_text(-15000, L"-1.5"));
    CHECK(cy_text(0xffffffffLL, L"429496.7295"));
    CHECK(cy_text(0x100000000LL, L"429496.7296"));
    CHECK(cy_text(0x7fffffffffffffffLL, L"922337203685477.5807"));
    CHECK(cy_text(9, L"0.0009"));
    BSTR b = (BSTR)1;
    CY cy0 = {0};
    CHECK(VarBstrFromCy(cy0, EN_US, 0, NULL) == E_INVALIDARG);

    /* VarCyFromR8 (test_VarCyFromR8). */
    LONGLONG c;
    CHECK(cy_from(-1.0, &c) == S_OK && c == -10000);
    CHECK(cy_from(1.5, &c) == S_OK && c == 15000);
    CHECK(cy_from(1.00009, &c) == S_OK && c == 10001);
    CHECK(cy_from(-0.00004, &c) == S_OK && c == 0);
    CHECK(cy_from(0.000251, &c) == S_OK && c == 3);
    CHECK(cy_from(4611686018427387648.0, &c) == DISP_E_OVERFLOW);

    /* VarI4FromStr / VarUI4FromStr (test_VarI4FromStr, test_VarUI4FromStr). */
    LONG l;
    ULONG ul;
    CHECK(i4(L"0", &l) == S_OK && l == 0);
    CHECK(i4(L"-2147483648", &l) == S_OK && l == (-2147483647 - 1));
    CHECK(i4(L"2147483647", &l) == S_OK && l == 2147483647);
    CHECK(i4(L"2147483648", &l) == DISP_E_OVERFLOW);
    CHECK(i4(L"-2147483649", &l) == DISP_E_OVERFLOW);
    CHECK(i4(L"-1.5", &l) == S_OK && l == -2);
    CHECK(i4(L"-0.6", &l) == S_OK && l == -1);
    CHECK(i4(L"0.5", &l) == S_OK && l == 0);
    CHECK(i4(L"1.5", &l) == S_OK && l == 2);
    CHECK(i4(L" 42 ", &l) == S_OK && l == 42);
    CHECK(i4(L"abc", &l) == DISP_E_TYPEMISMATCH);
    CHECK(i4(L"", &l) == DISP_E_TYPEMISMATCH);
    CHECK(ui4(L"4294967295", &ul) == S_OK && ul == 4294967295u);
    CHECK(ui4(L"4294967296", &ul) == DISP_E_OVERFLOW);
    CHECK(ui4(L"-1", &ul) == DISP_E_OVERFLOW);
    CHECK(ui4(L"-0.5", &ul) == S_OK && ul == 0);
    CHECK(ui4(L"0.6", &ul) == S_OK && ul == 1);

    /* VarR8FromStr, VarCyFromStr. */
    double r = 7;
    CHECK(VarR8FromStr(L"-1.5", EN_US, 0, &r) == S_OK && r == -1.5);
    CHECK(VarR8FromStr(L"0.6", EN_US, 0, &r) == S_OK && r == 0.6);
    CHECK(VarR8FromStr(L"1e3", EN_US, 0, &r) == S_OK && r == 1000.0);
    CHECK(VarR8FromStr(L" ", EN_US, 0, &r) == DISP_E_TYPEMISMATCH);
    CY cy;
    CHECK(VarCyFromStr(L"1.5", EN_US, 0, &cy) == S_OK && cy.int64 == 15000);
    CHECK(VarCyFromStr(L"1.23456", EN_US, 0, &cy) == S_OK && cy.int64 == 12346);
    CHECK(VarCyFromStr(L"2.00005", EN_US, 0, &cy) == S_OK && cy.int64 == 20000); /* half to even */
    CHECK(VarCyFromStr(L"x", EN_US, 0, &cy) == DISP_E_TYPEMISMATCH);

    /* VarBoolFromStr (test_VarBoolFromStr), VarBstrFromBool. */
    VARIANT_BOOL vb;
    CHECK(to_bool(L"#TRUE#", &vb) == S_OK && vb == VARIANT_TRUE);
    CHECK(to_bool(L"#FALSE#", &vb) == S_OK && vb == VARIANT_FALSE);
    CHECK(to_bool(L"#False#", &vb) == DISP_E_TYPEMISMATCH);
    CHECK(to_bool(L"false", &vb) == S_OK && vb == VARIANT_FALSE);
    CHECK(to_bool(L"TRUE", &vb) == S_OK && vb == VARIANT_TRUE);
    CHECK(to_bool(L"On", &vb) == DISP_E_TYPEMISMATCH);
    CHECK(to_bool(L"0", &vb) == S_OK && vb == VARIANT_FALSE);
    CHECK(to_bool(L"-1", &vb) == S_OK && vb == VARIANT_TRUE);
    CHECK(to_bool(L"0.1", &vb) == S_OK && vb == VARIANT_TRUE);
    b = NULL;
    CHECK(VarBstrFromBool(VARIANT_TRUE, EN_US, 0, &b) == S_OK && bstr_is(b, L"True"));
    SysFreeString(b);
    b = NULL;
    CHECK(VarBstrFromBool(VARIANT_FALSE, EN_US, 0, &b) == S_OK && bstr_is(b, L"False"));
    SysFreeString(b);

    /* VarDateFromStr (test_VarDateFromStr's time forms; en-US dates). */
    CHECK(date(L"1/2/1980", &dt) == S_OK && dt == 29222.0);
    CHECK(date(L"12/31/1999", &dt) == S_OK && dt == 36525.0);
    CHECK(date(L"1/2/1980 6:00 PM", &dt) == S_OK && dt == 29222.75);
    CHECK(date(L"1/2/1980 12:00:30", &dt) == S_OK && close_to(dt, 29222.5 + 30.0 / 86400));
    CHECK(date(L"1980-01-02", &dt) == S_OK && dt == 29222.0);
    CHECK(date(L"3:5", &dt) == S_OK && close_to(dt, 0.1284722222222222));
    CHECK(date(L"12:59 AM", &dt) == S_OK && close_to(dt, 0.04097222222222222));
    CHECK(date(L"00:00 pm", &dt) == S_OK && dt == 0.5);
    CHECK(date(L"13:00 PM", &dt) == S_OK && close_to(dt, 0.5416666666666666));
    CHECK(date(L"2 Jan 1970", &dt) == S_OK && dt == 25570.0);
    CHECK(date(L"2 January, 1970", &dt) == S_OK && dt == 25570.0);
    CHECK(date(L"1970-1-13", &dt) == S_OK && dt == 25581.0);
    CHECK(date(L"13-1-1970", &dt) == S_OK && dt == 25581.0); /* 13 cannot be the month */
    CHECK(date(L"6/30/2011 01:20:34 PM", &dt) == S_OK && close_to(dt, 40724.55594907407));
    CHECK(date(L"2013-05-14 02:04:12", &dt) == S_OK && close_to(dt, 41408.08625000001));
    CHECK(date(L"32 49", &dt) == DISP_E_TYPEMISMATCH);
    CHECK(date(L"", &dt) == DISP_E_TYPEMISMATCH);

    /* VarCmp (vartest.c test_VarCmp). */
    VARIANT x, y;
    VariantInit(&x);
    VariantInit(&y);
    V_VT(&x) = VT_I4; V_I4(&x) = 1; V_VT(&y) = VT_R8; V_R8(&y) = 1.0;
    CHECK(cmp(&x, &y) == VARCMP_EQ);
    V_VT(&x) = VT_I2; V_I2(&x) = 1; V_VT(&y) = VT_I2; V_I2(&y) = 2;
    CHECK(cmp(&x, &y) == VARCMP_LT && cmp(&y, &x) == VARCMP_GT);
    V_VT(&y) = VT_EMPTY;
    CHECK(cmp(&x, &y) == VARCMP_GT);
    V_VT(&y) = VT_NULL;
    CHECK(cmp(&x, &y) == VARCMP_NULL);
    BSTR s7 = SysAllocString(L"7"), s42 = SysAllocString(L"42"), empty = SysAllocString(L"");
    V_VT(&x) = VT_BSTR; V_BSTR(&x) = s7; V_VT(&y) = VT_BSTR; V_BSTR(&y) = s42;
    CHECK(cmp(&x, &y) == VARCMP_GT); /* as strings */
    V_BSTR(&y) = s7;
    CHECK(cmp(&x, &y) == VARCMP_EQ);
    V_VT(&y) = VT_I4; V_I4(&y) = 7;
    CHECK(cmp(&x, &y) == VARCMP_GT && cmp(&y, &x) == VARCMP_LT); /* a string is greater */
    V_VT(&y) = VT_EMPTY;
    CHECK(cmp(&x, &y) == VARCMP_GT);
    V_BSTR(&x) = empty;
    CHECK(cmp(&x, &y) == VARCMP_EQ);
    V_BSTR(&x) = NULL;
    CHECK(cmp(&x, &y) == VARCMP_EQ);
    V_VT(&x) = VT_ERROR; V_ERROR(&x) = 0; V_VT(&y) = VT_UI1; V_UI1(&y) = 0;
    CHECK(cmp(&x, &y) == DISP_E_TYPEMISMATCH);
    SysFreeString(s7);
    SysFreeString(s42);
    SysFreeString(empty);

    /* GetActiveObject: nothing is running. */
    static const CLSID nothing = {0x6a0c8d1e, 0x3f2b, 0x4c55, {0x9e, 0x10, 0x5a, 0x77, 0x12, 0x34, 0x56, 0x78}};
    IUnknown *unk = (IUnknown *)1;
    CHECK(GetActiveObject(&nothing, NULL, &unk) == MK_E_UNAVAILABLE && unk == NULL);

    printf("oleaut32_test: ok\n");
    return 0;
}
