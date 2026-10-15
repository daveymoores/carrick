import { greet } from '@fixture/shared';

declare global {
  namespace JSX {
    interface IntrinsicElements {
      [name: string]: { [attribute: string]: unknown };
    }
  }
}

export function Greeting(props: { name: string }) {
  return <p title={props.name}>{greet(props.name)}</p>;
}
