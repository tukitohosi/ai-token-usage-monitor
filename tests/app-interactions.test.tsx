// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../src/App";
import { MockDashboardAdapter } from "../src/ui/dashboard-adapter";

beforeEach(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("dashboard interactions", () => {
  it("keeps explicit names on every primary navigation button", async () => {
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await screen.findByRole("heading", { name: "总览" });
    for (const name of ["总览", "本机活动", "费用", "设置"]) {
      expect(screen.getByRole("button", { name }).getAttribute("aria-label")).toBe(name);
    }
  });

  it("shows three identified quota cards and two local metric cards", async () => {
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await screen.findByRole("heading", { name: "总览" });
    expect(screen.getByText("Codex 5 小时")).toBeTruthy();
    expect(screen.getByText("Codex 7 天")).toBeTruthy();
    expect(screen.getByText("GPT-5.6 Luna 储备 7 天")).toBeTruthy();
    expect(document.querySelectorAll(".overview-grid__quota")).toHaveLength(3);
    expect(document.querySelectorAll(".overview-grid__local")).toHaveLength(2);
  });

  it("shows backend price rates and marks unpublished cache writes unpriced", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: "费用" }));
    await user.click(screen.getByRole("button", { name: "查看定价口径" }));
    expect(screen.getByText("GPT-6 Astra")).toBeTruthy();
    expect(screen.getByText("DeepSeek V4.1 Flash")).toBeTruthy();
    expect(screen.getByText("Tencent Hy4 preview")).toBeTruthy();
    expect(screen.getAllByText("未公布/未计价").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText("¥18")).toBeTruthy();
  });

  it("supports selecting multiple AI sources", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);

    await user.click(await screen.findByRole("button", { name: /本机活动/ }));
    await user.click(screen.getByRole("button", { name: "7 天" }));
    const modeControl = screen.getByRole("group", { name: "AI 来源选择模式" });
    const singleMode = within(modeControl).getByRole("button", { name: "单选" });
    const multipleMode = within(modeControl).getByRole("button", { name: "多选" });
    const allSources = screen.getByRole("button", { name: "全部来源" });
    const codex = screen.getByRole("button", { name: "Codex，状态：已就绪" });
    const claude = screen.getByRole("button", { name: "Claude Code，状态：已就绪" });

    expect(singleMode.getAttribute("aria-pressed")).toBe("true");
    expect(multipleMode.getAttribute("aria-pressed")).toBe("false");
    expect(allSources.classList.contains("is-active")).toBe(true);
    await user.click(multipleMode);
    await user.click(codex);
    await user.click(claude);

    expect(screen.getByRole("button", { name: "7 天" }).getAttribute("aria-pressed")).toBe("true");
    expect(singleMode.getAttribute("aria-pressed")).toBe("false");
    expect(multipleMode.getAttribute("aria-pressed")).toBe("true");
    expect(codex.getAttribute("aria-pressed")).toBe("true");
    expect(claude.getAttribute("aria-pressed")).toBe("true");
    expect(codex.classList.contains("is-active")).toBe(true);
    expect(claude.classList.contains("is-active")).toBe(true);
    expect(allSources.classList.contains("is-active")).toBe(false);
    expect(screen.getByText(/当前：最近 7 天 · Codex、Claude Code（已选 2 个来源）/)).toBeTruthy();
  });

  it("keeps the latest selected source when switching from multiple to single", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));

    const modeControl = screen.getByRole("group", { name: "AI 来源选择模式" });
    const singleMode = within(modeControl).getByRole("button", { name: "单选" });
    await user.click(within(modeControl).getByRole("button", { name: "多选" }));
    const codex = screen.getByRole("button", { name: "Codex，状态：已就绪" });
    const claude = screen.getByRole("button", { name: "Claude Code，状态：已就绪" });
    await user.click(codex);
    await user.click(claude);
    await user.click(singleMode);

    expect(singleMode.getAttribute("aria-pressed")).toBe("true");
    expect(codex.getAttribute("aria-pressed")).toBe("false");
    expect(claude.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByText(/当前：今日 · Claude Code（已选 1 个来源）/)).toBeTruthy();
  });

  it("explains source health in accessible names and hover text", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));

    const workBuddyAi = screen.getByRole("button", { name: "WorkBuddy AI，状态：未检测到" });
    const cursor = screen.getByRole("button", { name: "Cursor，状态：暂不支持" });
    expect(workBuddyAi.getAttribute("title")).toBe("WorkBuddy AI · 状态：未检测到");
    expect(cursor.getAttribute("title")).toBe("Cursor · 状态：暂不支持");
  });

  it("keeps activity and cost source choices separate", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));
    expect(screen.getByRole("list", { name: "过去 24 小时使用量" }).querySelectorAll('[role="listitem"]')).toHaveLength(24);
    await user.click(screen.getByRole("button", { name: "Claude Code，状态：已就绪" }));
    await user.click(screen.getByRole("button", { name: /费用/ }));
    expect(screen.getByRole("button", { name: "全部来源" }).getAttribute("aria-pressed")).toBe("true");
    await user.click(screen.getByRole("button", { name: "Codex，状态：已就绪" }));
    await user.click(within(screen.getByRole("group", { name: "AI 来源选择模式" })).getByRole("button", { name: "多选" }));
    await user.click(screen.getByRole("button", { name: "Claude Code，状态：已就绪" }));
    expect(screen.getByRole("button", { name: "Codex，状态：已就绪" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "Claude Code，状态：已就绪" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("uses dedicated wrappers for empty model and project attribution states", async () => {
    class EmptyAttributionAdapter extends MockDashboardAdapter {
      override async refresh() {
        const snapshot = await super.refresh();
        for (const source of snapshot.deviceUsage?.sources ?? []) {
          if (!source.ranges?.today) continue;
          source.ranges.today.byModel = [];
          source.ranges.today.byProject = [];
        }
        return snapshot;
      }
    }
    const user = userEvent.setup();
    render(<App adapter={new EmptyAttributionAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));

    const emptyTitles = screen.getAllByText("暂无归因数据");
    expect(emptyTitles).toHaveLength(2);
    for (const title of emptyTitles) {
      const emptyState = title.closest(".empty-state");
      expect(emptyState?.querySelector(".empty-state__icon")).not.toBeNull();
      expect(emptyState?.querySelector(".empty-state__content")).not.toBeNull();
      expect(emptyState?.textContent).toContain("当前范围内没有可归属的模型或项目。");
    }
  });

  it("does not present unavailable cost sources as confirmed zero cost", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: "费用" }));

    expect(screen.getAllByText("WorkBuddy AI").at(-1)?.closest("li")?.textContent).toContain("暂无数据");
    expect(screen.getAllByText("Cursor").at(-1)?.closest("li")?.textContent).toContain("不可用");
  });

  it("moves focus into the day drawer, closes with Escape, and restores focus", async () => {
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await screen.findByRole("heading", { name: "总览" });
    const trendButton = document.querySelector<HTMLButtonElement>(".stacked-trend__bar");
    expect(trendButton).not.toBeNull();
    trendButton!.focus();
    fireEvent.click(trendButton!);

    const dialog = await screen.findByRole("dialog");
    const close = screen.getByRole("button", { name: "关闭日用量明细" });
    expect(dialog).toBeTruthy();
    expect(document.activeElement).toBe(close);
    expect(document.body.style.overflow).toBe("hidden");
    await userEvent.setup().click(screen.getByRole("button", { name: "项目" }));
    expect(screen.getByRole("heading", { name: "项目用量汇总" })).toBeTruthy();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.body.style.overflow).toBe("");
    expect(document.activeElement).toBe(trendButton);
  });

  it("shows fifteen local daily slots and the backend refresh countdown", async () => {
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    expect(await screen.findByRole("heading", { name: "Codex 账号近期每日 Token" })).toBeTruthy();
    expect(screen.getByRole("list", { name: "Codex 账号近期每日 Token" }).querySelectorAll('[role="listitem"]')).toHaveLength(7);
    expect(await screen.findByRole("heading", { name: "本机近 15 日 Token" })).toBeTruthy();
    expect(screen.getByRole("list", { name: "本机近 15 日 Token" }).querySelectorAll('[role="listitem"]')).toHaveLength(15);
    expect(screen.getByText(/约 \d+ 秒后自动更新/)).toBeTruthy();
  });

  it("keeps zero-token hourly slots without drawing visible bars and labels yesterday", async () => {
    const dateNow = vi.spyOn(Date, "now").mockReturnValue(new Date(2026, 8, 1, 1, 30).getTime());
    class ZeroHourlyAdapter extends MockDashboardAdapter {
      override async refresh() {
        const snapshot = await super.refresh();
        for (const source of snapshot.deviceUsage?.sources ?? []) {
          if (source.ranges?.last24Hours) source.ranges.last24Hours.hourlyUsage = [];
        }
        return snapshot;
      }
    }
    const user = userEvent.setup();
    render(<App adapter={new ZeroHourlyAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));
    const chart = screen.getByRole("list", { name: "过去 24 小时使用量" });
    expect(chart.querySelectorAll('[role="listitem"]')).toHaveLength(24);
    expect(chart.querySelectorAll(".hourly-trend__stack")).toHaveLength(0);
    expect(chart.textContent).toContain("昨天");
    expect(chart.querySelectorAll("small")).toHaveLength(24);
    const labels = [...chart.querySelectorAll("small")].filter((label) => label.textContent !== "");
    expect(labels).toHaveLength(13);
    expect(labels.slice(1, -1).every((label) => Number(label.textContent!.match(/(\d{2}):00/)?.[1]) % 2 === 0)).toBe(true);
    dateNow.mockRestore();
  });

  it("creates, edits, persists, and removes a cross-source project merge", async () => {
    const user = userEvent.setup();
    const adapter = new MockDashboardAdapter("ready", 0);
    const save = vi.spyOn(adapter, "setProjectMergeRules");
    render(<App adapter={adapter} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));
    await user.click(screen.getByRole("button", { name: "管理合并" }));

    const dialog = screen.getByRole("dialog", { name: "项目合并管理" });
    const choices = [...within(dialog).getByRole("group", { name: "选择要合并的项目" }).querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(choices.length).toBeGreaterThanOrEqual(2);
    await user.click(choices[0]);
    await user.click(choices[1]);
    await user.click(within(dialog).getByRole("button", { name: "创建合并" }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(await adapter.readProjectMergeRules()).toHaveLength(1);
    expect(within(dialog).getByText("已有合并")).toBeTruthy();

    await user.click(within(dialog).getByRole("button", { name: "编辑" }));
    expect(within(dialog).getByRole("button", { name: "保存修改" })).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "退出编辑" }));
    await user.click(within(dialog).getByRole("button", { name: "取消合并" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(await adapter.readProjectMergeRules()).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "关闭项目合并管理" }));
    expect(screen.queryByRole("dialog", { name: "项目合并管理" })).toBeNull();
  });

  it("applies a custom local date range and exposes model drilldown", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /本机活动/ }));
    const start = screen.getByLabelText("开始日期");
    const end = screen.getByLabelText("结束日期");
    fireEvent.change(start, { target: { value: "2026-09-01" } });
    fireEvent.change(end, { target: { value: "2026-09-01" } });
    await user.click(screen.getByRole("button", { name: "应用日期" }));

    expect(await screen.findByText(/当前：2026-09-01 至 2026-09-01/)).toBeTruthy();
    fireEvent.change(start, { target: { value: "2026-08-31" } });
    expect(screen.getByText(/当前：2026-09-01 至 2026-09-01/)).toBeTruthy();
    const drilldown = screen.getAllByRole("button", { name: /查看模型 .* 的归因汇总/ })[0];
    await user.click(drilldown);
    expect(screen.getByText("模型下钻")).toBeTruthy();
    expect(screen.getByRole("button", { name: "清除下钻" })).toBeTruthy();
  });

  it("allows a user to return from an explicit theme to follow-system mode", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /设置/ }));
    const system = screen.getByRole("button", { name: "跟随系统" });
    const dark = screen.getByRole("button", { name: "深色" });

    await user.click(dark);
    expect(dark.getAttribute("aria-pressed")).toBe("true");
    await user.click(system);
    expect(system.getAttribute("aria-pressed")).toBe("true");
  });

  it("protects unsaved runtime settings when leaving the settings view", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: "设置" }));
    const refreshInterval = screen.getAllByRole("combobox")[1] as HTMLSelectElement;
    const save = screen.getByRole("button", { name: "保存运行设置" }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    await user.selectOptions(refreshInterval, "5");
    expect(save.disabled).toBe(false);
    await user.click(screen.getByRole("button", { name: "本机活动" }));
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "继续编辑" }));
    expect((screen.getAllByRole("combobox")[1] as HTMLSelectElement).value).toBe("5");

    await user.click(screen.getByRole("button", { name: "本机活动" }));
    await user.click(screen.getByRole("button", { name: "放弃更改" }));
    expect(await screen.findByRole("heading", { name: "本机活动" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect((screen.getAllByRole("combobox")[1] as HTMLSelectElement).value).toBe("1");
  });

  it("tests notifications and exposes the latest successful trigger", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /设置/ }));
    await user.click(screen.getByRole("button", { name: "发送测试通知" }));
    expect(await screen.findByText(/测试通知已发送/)).toBeTruthy();
    expect(screen.getByText(/最近通知：/)).toBeTruthy();
    expect(screen.getByText("测试通知：系统通知链路可用")).toBeTruthy();
    const notificationSwitch = screen.getByText("系统通知").closest(".setting-row")?.querySelector("input");
    expect(notificationSwitch).not.toBeNull();
    await user.click(notificationSwitch!);
    expect((screen.getByRole("button", { name: "发送测试通知" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("requires a second confirmation before rebuilding derived indexes", async () => {
    const user = userEvent.setup();
    const adapter = new MockDashboardAdapter("ready", 0);
    const rebuild = vi.spyOn(adapter, "rebuildIndexes");
    render(<App adapter={adapter} />);
    await user.click(await screen.findByRole("button", { name: /设置/ }));
    expect(await screen.findByText("索引状态正常")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "备份后重建" }));
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(rebuild).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "备份后重建" }));
    await user.click(screen.getByRole("button", { name: "确认备份并重建" }));
    await waitFor(() => expect(rebuild).toHaveBeenCalledOnce());
    expect(await screen.findByText(/派生索引已备份并重建完成/)).toBeTruthy();
  });

  it("shows a green success notification after a healthy index self-check", async () => {
    const user = userEvent.setup();
    render(<App adapter={new MockDashboardAdapter("ready", 0)} />);
    await user.click(await screen.findByRole("button", { name: /设置/ }));
    await user.click(screen.getByRole("button", { name: "立即自检" }));
    expect(await screen.findByText("索引自检通过，未发现问题。")).toBeTruthy();
  });
});
