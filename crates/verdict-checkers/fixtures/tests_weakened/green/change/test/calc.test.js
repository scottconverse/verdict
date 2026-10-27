const { add } = require('../src/calc');

test('adds', () => {
  expect(add(1, 2)).toBe(3);
  expect(add(2, 2)).toBe(4);
});

test('negatives', () => {
  expect(add(-1, -2)).toBe(-3);
});
