type View = { on(event: string, listener: () => void): void };

function attach(view: View): void {
  view.on("blur", () => {
    console.log("blurred");
  });
}

export function mount(view: View): void {
  attach(view);
}
