// The API coverage section: how much of Windows' API gala's DLLs export,
// and how much of what real programs import. The figures are data
// (`coverage.json`), written by `tools/api_coverage.py` (WORKFLOW "API
// coverage"); this file only draws them.
import data from "./coverage.json";
import { Both, localize, useLang, useT } from "./i18n";

export type Coverage = typeof data;
export const coverage: Coverage = data;

/** `part` of `whole` as a percentage, to `digits` places, rounded down:
 * 100% only when nothing is missing. */
export function pct(part: number, whole: number, digits = 0): string {
  const scale = 10 ** digits;
  return whole === 0 ? "0" : (Math.floor((100 * scale * part) / whole) / scale).toFixed(digits);
}

const n = (x: number) => x.toLocaleString("en-US");

/** A bar: what gala exports for real, filled; its stubs, hatched. */
function Bar({ real, stubs, of }: { real: number; stubs: number; of: number }) {
  return (
    <span className="cov-bar" aria-hidden="true">
      <span className="cov-real" style={{ inlineSize: `${pct(real, of, 2)}%` }} />
      <span className="cov-stub" style={{ inlineSize: `${pct(stubs, of, 2)}%` }} />
    </span>
  );
}

/** How many of the Windows DLLs gala has are drawn, the largest first. */
const DLLS_SHOWN = 24;

export function CoverageBody() {
  const t = useT();
  const lang = useLang();
  const c = coverage;
  const whole = pct(c.gala.real, c.windows.exports, 1);
  return (
    <>
      <div className="cov-head">
        <p className="cov-big">
          <b>{whole}%</b>
          <span>
            <Both
              en={
                <>
                  of Windows&rsquo; {n(c.windows.exports)} named functions: gala&rsquo;s {c.gala.dlls} DLLs export{" "}
                  {n(c.gala.real)} of them for real, and {n(c.gala.stubs)} more as stubs that only report failure.
                </>
              }
              ar={
                <>
                  من {n(c.windows.exports)} دالة مسمّاة في Windows: تصدّر مكتبات gala الـ {c.gala.dlls}{" "}
                  {n(c.gala.real)} منها تنفيذًا فعليًا، و{n(c.gala.stubs)} أخرى بدائل لا تفعل سوى الإبلاغ عن الفشل.
                </>
              }
            />
          </span>
        </p>
        <p className="cov-note">
          <Both
            en={
              <>
                The whole is every named export of the {n(c.windows.dlls)} 64-bit DLLs in {c.windows.product}&rsquo;s
                System32 (build {c.windows.build}), legacy included: much of it is no program&rsquo;s concern, and gala
                is not meant to reach it. What a program needs is the measure that matters, below.
              </>
            }
            ar={
              <>
                الكلّ هو كل دالة مسمّاة تصدّرها مكتبات DLL الـ {n(c.windows.dlls)} ذات 64 بت في مجلد System32 لـ{" "}
                {c.windows.product} (الإصدار {c.windows.build})، بما فيها القديم: كثير منه لا يعني أي برنامج، وليس
                على gala بلوغه. أما ما يحتاج إليه البرنامج فهو المقياس الذي يهمّ، أدناه.
              </>
            }
          />
        </p>
      </div>

      <h3 className="sub">{t("What programs import")}</h3>
      <p className="cov-intro">
        {t(
          "The Windows functions each program imports, from its executables and the DLLs of its own they load, and how many of those gala's DLLs export. The bar counts all of them; the figure after it, only the static imports, which Windows resolves before the program runs (the rest are delay imports, each resolved the first time it is called).",
        )}
      </p>
      <ul className="cov-list">
        {c.programs.map((p) => (
          <li key={p.name}>
            <span className="cov-name">
              {localize(lang, p.name, "name")}
              {"version" in p && p.version ? <span> {String(p.version)}</span> : null}
            </span>
            <Bar real={p.real} stubs={p.stubs} of={p.imports} />
            <span className="cov-fig">
              <b>{pct(p.real, p.imports)}%</b>
              <span>
                <Both
                  en={
                    <>
                      {n(p.real)} of {n(p.imports)}; of the static ones, {pct(p.static.real, p.static.imports)}% of{" "}
                      {n(p.static.imports)}
                    </>
                  }
                  ar={
                    <>
                      {n(p.real)} من {n(p.imports)}؛ ومن الثابتة {pct(p.static.real, p.static.imports)}% من{" "}
                      {n(p.static.imports)}
                    </>
                  }
                />
              </span>
            </span>
          </li>
        ))}
      </ul>

      <h3 className="sub">{t("Windows' DLLs, one by one")}</h3>
      <p className="cov-intro">
        {t(
          "The largest of the Windows DLLs gala has its own of, each against every function the Windows one exports. A DLL gala does not have counts as none of its functions in the whole above.",
        )}
      </p>
      <ul className="cov-list cov-dlls">
        {c.dlls.slice(0, DLLS_SHOWN).map((d) => (
          <li key={d.name}>
            <code className="cov-name">{d.name}</code>
            <Bar real={d.real} stubs={d.stubs} of={d.windows} />
            <span className="cov-fig">
              <b>{pct(d.real, d.windows)}%</b>
              <span>
                <Both
                  en={
                    <>
                      {n(d.real)} of {n(d.windows)}
                    </>
                  }
                  ar={
                    <>
                      {n(d.real)} من {n(d.windows)}
                    </>
                  }
                />
              </span>
            </span>
          </li>
        ))}
      </ul>

      <p className="cov-legend">
        <span className="cov-key cov-key-real" /> {t("exported for real")}
        <span className="cov-key cov-key-stub" /> {t("a stub that reports failure")}
      </p>

      <h3 className="sub">{t("What these figures do not see")}</h3>
      <ul className="cov-caveats">
        <li>
          {t(
            "Functions a program looks up while it runs (GetProcAddress) are in no import table, and neither are the methods of COM interfaces: a program can import everything it names and still stop on one of those.",
          )}
        </li>
        <li>
          {t(
            "An export that exists may do only part of its job. These figures say what a program finds when it loads, not that it works: the boot scenarios in What runs say that.",
          )}
        </li>
        <li>
          {t(
            "They are read from the files' import and export tables, metadata every Windows binary carries, never from Microsoft's code.",
          )}
        </li>
      </ul>
      <p className="cov-when">
        <Both
          en={
            <>
              Measured {c.measured}, gala at commit <code>{c.galaCommit}</code>.
            </>
          }
          ar={
            <>
              قيس في {c.measured}، وgala عند الإيداع <code>{c.galaCommit}</code>.
            </>
          }
        />
      </p>
    </>
  );
}
