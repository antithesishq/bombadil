function make(value: number): number {
  return value;
}

const constCall = make(1);
let letCall = make(2);
var varCall = make(3);
export const exportedCall = make(4);
const notCall = 5;
const { length } = String(make(6));

function nested() {
  const inner = make(7);
  return inner;
}

export { constCall, letCall, varCall, notCall, length, nested };
