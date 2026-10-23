import { expect, open, test } from "nsh/test";

test("shows the Liber library shell", async () => {
  const app = await open(".");
  expect(app.texts()).toContain("Liber");
  expect(app.texts()).toContain("⌂   Главная");
  expect(app.texts()).toContain("＋   Добавить PDF");
  expect(app.texts()).toContain("Читаю сейчас");
});
