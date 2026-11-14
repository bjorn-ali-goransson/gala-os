import type { ReactNode } from "react";

export function Section({
  id,
  title,
  intro,
  children,
}: {
  id: string;
  title: string;
  intro?: string;
  children: ReactNode;
}) {
  return (
    <section className="sec" id={id} aria-labelledby={`${id}-h`}>
      <h2 id={`${id}-h`}>{title}</h2>
      {intro && <p className="sec-intro">{intro}</p>}
      {children}
    </section>
  );
}
