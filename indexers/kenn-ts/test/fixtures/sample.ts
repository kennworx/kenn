// Copyright 2026 Example Corp
/** @fileoverview fixture for kenn-ts tests */
import { join } from "node:path";

export enum Color { Red, Green }
export interface Shape { area(): number; label: string; }
export interface Shape { extra: boolean; }
export class Circle implements Shape {
  label = "c";
  extra = false;
  area(): number { this.label = "d"; return this.label.length; }
}
export function identity<T>(x: T): T { return x; }
export function describe(s: Shape): string { return s.label; }
export const p = join("a", "b");
