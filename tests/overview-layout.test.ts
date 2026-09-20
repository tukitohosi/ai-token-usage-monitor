import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

describe("overview responsive layout contract", () => {
  it("places three quota cards above two local metrics on wide screens", () => {
    expect(styles).toMatch(/\.overview-grid\s*\{[^}]*repeat\(6/);
    expect(styles).toMatch(/\.overview-grid__quota\s*\{[^}]*span 2/);
    expect(styles).toMatch(/\.overview-grid__local\s*\{[^}]*span 3/);
  });

  it("uses two tablet columns and one mobile column", () => {
    const tablet = styles.slice(styles.indexOf("@media (max-width: 900px)"));
    const mobile = styles.slice(styles.indexOf("@media (max-width: 820px)"));
    expect(tablet).toMatch(/\.overview-grid, \.cost-kpis\s*\{[^}]*repeat\(2/);
    expect(tablet).toMatch(/\.overview-grid__quota, \.overview-grid__local\s*\{[^}]*grid-column: auto/);
    expect(mobile).toMatch(/\.overview-grid, \.cost-kpis[^}]*grid-template-columns: minmax\(0,1fr\)/);
  });
});
