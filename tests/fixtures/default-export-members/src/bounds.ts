// The bounds. An object the module keeps to itself is a value it uses, not a
// surface it offers.
const internal = {
  tidy(input: string): string {
    return input.trim();
  },
};

// A `let` can be reassigned after it is exported, so the object it starts as
// is not what the module offers.
let state = {
  reset(): number {
    return 0;
  },
};
state = { reset: () => 1 };

export { state };

export function clean(input: string): string {
  // A `const` inside a function is the function's, not the module's, even
  // when it shares an exported name.
  const state = {
    hidden(): string {
      return input;
    },
  };
  return internal.tidy(state.hidden());
}
