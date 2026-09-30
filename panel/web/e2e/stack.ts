import { execFileSync, spawn } from "node:child_process";
import type { ChildProcess } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

/** What global-setup made: a running panel, and how to reach and drive it. */
export interface Stack {
  /** `https://127.0.0.1:PORT/SECRET/` */
  base: string;
  cfg: string;
  bin: string;
  root: string;
  pids: number[];
}

const FILE = path.join(os.tmpdir(), "kariz-e2e-stack.json");

export const exe = process.platform === "win32" ? ".exe" : "";

export function writeStack(stack: Stack) {
  fs.writeFileSync(FILE, JSON.stringify(stack));
}

export function readStack(): Stack {
  return JSON.parse(fs.readFileSync(FILE, "utf8")) as Stack;
}

export function removeStack() {
  fs.rmSync(FILE, { force: true });
}

/** A port nothing listens on now. */
export function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address() as net.AddressInfo;
      server.close(() => resolve(port));
    });
  });
}

export async function waitForPort(port: number, ms = 20_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const ok = await new Promise<boolean>((resolve) => {
      const s = net.connect(port, "127.0.0.1");
      s.once("connect", () => {
        s.destroy();
        resolve(true);
      });
      s.once("error", () => resolve(false));
    });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`nothing listens on ${port}`);
}

/** A one-time sign-in link, made the way an admin makes one on the server. */
export function newLink(): string {
  const s = readStack();
  const out = execFileSync(s.bin, ["login-link", "-c", s.cfg, "--host", "127.0.0.1"], { encoding: "utf8" });
  const link = out.split(/\r?\n/).find((l) => l.startsWith("https://"));
  if (!link) throw new Error(`no link in: ${out}`);
  return link;
}

/** Sets the admin password (which signs every session out). */
export function setPassword(password: string) {
  const s = readStack();
  execFileSync(s.bin, ["reset-password", "-c", s.cfg, "--stdin"], { input: `${password}\n` });
}

export interface Agent {
  process: ChildProcess;
  dir: string;
  stop: () => void;
}

/** A real agent on this machine, joined with a code from the panel, running its tunnels as
 *  child processes. */
export function startAgent(code: string): Agent {
  const s = readStack();
  const dir = fs.mkdtempSync(path.join(s.root, "agent-"));
  const cfg = path.join(dir, "agent.toml");
  const kariz = path.join(dir, "kariz");
  fs.mkdirSync(kariz, { recursive: true });
  execFileSync(s.bin, ["agent", "-c", cfg, "--join", code, "--no-run"]);
  const text = fs
    .readFileSync(cfg, "utf8")
    .replace(/^kariz_dir = .*$/m, `kariz_dir = ${JSON.stringify(kariz)}`)
    .replace(/^services = .*$/m, 'services = "process"');
  fs.writeFileSync(cfg, text);
  const child = spawn(s.bin, ["agent", "-c", cfg], { stdio: ["ignore", fs.openSync(path.join(dir, "agent.log"), "a"), "inherit"] });
  return { process: child, dir, stop: () => child.kill() };
}
