// Run the plugin's SessionStart hook, exactly as hooks.json registers it,
// against stand-ins for the workforest CLI: it must warn when the CLI is
// missing, reports no version, or is older than the skill needs, and print
// nothing when the CLI is new enough.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const plugin = fileURLToPath(new URL("../plugin", import.meta.url));
const read = (path) => readFileSync(join(plugin, path), "utf8");

const [{ command }] = JSON.parse(read("hooks/hooks.json")).hooks.SessionStart.flatMap((group) => group.hooks);
const required = read("hooks/check-cli.sh").match(/^required=(\S+)$/m)[1];
const skill = read("skills/workforest/SKILL.md");
let failed = false;
const fail = (message) => {
  console.error(message);
  failed = true;
};

// The hook names the skill's version and points Claude at its install section.
if (skill.match(/version (\S+) or later/)?.[1] !== required) {
  fail(`the hook requires ${required}, but SKILL.md doesn't say "version ${required} or later"`);
}
if (!/^## Before first use$/m.test(skill)) fail('SKILL.md has no "Before first use" section');

const [major, minor, patch] = required.split(".").map(Number);
const older =
  patch > 0 ? `${major}.${minor}.${patch - 1}` : minor > 0 ? `${major}.${minor - 1}.99` : `${major - 1}.99.99`;
const cases = [
  { name: "no workforest on PATH", cli: null, warns: /isn't installed/ },
  {
    name: "a workforest that rejects --version",
    cli: 'echo "workforest: unknown command: --version (try --help)" >&2; exit 1',
    warns: /isn't the CLI this plugin needs/,
  },
  {
    name: "another program named workforest",
    cli: "echo 'forest-tools 2.0.0'",
    warns: /isn't the CLI this plugin needs/,
  },
  { name: `workforest ${older}`, cli: `echo 'workforest ${older}'`, warns: new RegExp(`${older} is older`) },
  { name: `workforest ${required}`, cli: `echo 'workforest ${required}'` },
  { name: `workforest ${required} from Nix`, cli: `echo 'workforest ${required} (abc1234-dirty)'` },
  { name: "a newer patch", cli: `echo 'workforest ${major}.${minor}.${patch + 10}'` },
  { name: "a newer minor", cli: `echo 'workforest ${major}.${minor + 10}.0'` },
  { name: "a newer major", cli: `echo 'workforest ${major + 1}.0.0'` },
];

// The hook may use only these, so the stand-in is the only workforest it finds.
const tmp = mkdtempSync(join(tmpdir(), "workforest-hook-"));
const tools = join(tmp, "tools");
mkdirSync(tools);
const which = (tool) => spawnSync("sh", ["-c", `command -v ${tool}`], { encoding: "utf8" }).stdout.trim();
const sh = which("sh");
for (const tool of ["sh", "sed", "awk"]) symlinkSync(which(tool), join(tools, tool));

for (const { name, cli, warns } of cases) {
  const bin = join(tmp, name.replace(/\W+/g, "-"));
  mkdirSync(bin);
  if (cli !== null) writeFileSync(join(bin, "workforest"), `#!${sh}\n${cli}\n`, { mode: 0o755 });
  const run = spawnSync(sh, ["-c", command], {
    env: { PATH: `${bin}:${tools}`, CLAUDE_PLUGIN_ROOT: plugin },
    encoding: "utf8",
  });
  const problems = [];
  if (run.status !== 0) problems.push(`exited ${run.status}`);
  if (run.stderr) problems.push(`wrote to stderr: ${run.stderr}`);
  if (!warns) {
    if (run.stdout) problems.push(`printed ${run.stdout}`);
  } else {
    let output;
    try {
      output = JSON.parse(run.stdout);
    } catch {
      problems.push(`printed something other than JSON: ${run.stdout}`);
    }
    if (output) {
      const message = output.systemMessage ?? "";
      const { hookEventName, additionalContext = "" } = output.hookSpecificOutput ?? {};
      if (!message.startsWith("workforest plugin: ") || !warns.test(message)) {
        problems.push(`systemMessage doesn't match ${warns}: ${message}`);
      }
      if (hookEventName !== "SessionStart") problems.push(`hookEventName is ${hookEventName}`);
      if (!additionalContext.includes("In your first reply") || !additionalContext.includes("'Before first use'")) {
        problems.push(`additionalContext doesn't point Claude at the install: ${additionalContext}`);
      }
    }
  }
  if (problems.length > 0) fail(`${name}:\n  ${problems.join("\n  ")}`);
  else console.log(`${name}: ${warns ? "warns" : "silent"}`);
}

rmSync(tmp, { recursive: true, force: true });
process.exit(failed ? 1 : 0);
