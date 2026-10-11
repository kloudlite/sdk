/**
 * Skill paths pi loads (`index.ts`). The skill files live in `../claude-plugin/skills/`; the
 * directory name is historical, only the skill markdown matters.
 */
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const SKILLS = fileURLToPath(new URL("../claude-plugin/skills", import.meta.url));
export const CODEMODE_SKILL = join(SKILLS, "codemode");
export const KLOUDLITE_SKILL = join(SKILLS, "kloudlite");
// Terse replies: output tokens were the waste (2026-10-12). Vendored from github.com/JuliusBrussee/caveman (MIT).
export const CAVEMAN_SKILL = join(SKILLS, "caveman");
export type Role = "main" | "workspace" | "subagent";
export const roleSkill = (role: Role) => join(SKILLS, `${role}-session`);
