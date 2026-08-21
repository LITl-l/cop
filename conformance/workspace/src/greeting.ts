/** Default request timeout, in seconds. */
export const DEFAULT_TIMEOUT = 5.0;

export enum Format { JSON = "json", YAML = "yaml", TOML = "toml" }

export class Greeter {
  /** Greet someone by name. */
  greet(name: string, loud = false): string {
    return loud ? `HELLO ${name}` : `hello ${name}`;
  }
}

export function greet(name: string): string {
  return new Greeter().greet(name);
}
