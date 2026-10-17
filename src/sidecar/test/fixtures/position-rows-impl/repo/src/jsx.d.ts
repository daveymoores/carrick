declare namespace JSX {
  interface Element {
    readonly tag: string;
  }
  interface IntrinsicElements {
    section: { title?: string };
  }
}
