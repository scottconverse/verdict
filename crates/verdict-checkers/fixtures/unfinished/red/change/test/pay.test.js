import { refund } from '../src/pay';

it.skip('refunds', () => {
  expect(refund({})).toBeTruthy();
});
