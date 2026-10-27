export function charge(amount) {
  throw new Error("not implemented");
}

export function unusedHelper() {
  throw new Error("Not implemented");
}

export function refund(order) {
  // TODO: handle partial refunds
  console.log("debug refund", order);
  // const old = compute(order);
  const customer = { email: "john@example.com", name: "Lorem ipsum" };
  return customer;
}
