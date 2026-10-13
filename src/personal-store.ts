// Local JSON persistence for the personal model (Node). Atomic write via temp file + rename.
import { readFileSync, writeFileSync, renameSync, existsSync, rmSync } from "node:fs";
import { PersonalModel, type PersonalState, type PersonalConfig } from "./personal.ts";
import type { FlipModel } from "./classifier.ts";

export function loadPersonal(base: FlipModel, path: string, config?: Partial<PersonalConfig>): PersonalModel {
  if (!existsSync(path)) return new PersonalModel(base, undefined, config);
  const st = JSON.parse(readFileSync(path, "utf8")) as PersonalState;
  if (st.version !== 1 || st.dw.length !== base.stage2.w.length) return new PersonalModel(base, undefined, config); // incompatible -> fresh
  return new PersonalModel(base, st);
}
export function savePersonal(m: PersonalModel, path: string): number {
  m.compact();
  const s = JSON.stringify(m.toJSON());
  writeFileSync(path + ".tmp", s); renameSync(path + ".tmp", path);
  return s.length;
}
/** Reset API: clears the in-memory delta + memory and deletes the file. */
export function resetPersonal(m: PersonalModel, path: string): void { m.reset(); if (existsSync(path)) rmSync(path); }
