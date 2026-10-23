import { expect, open, test } from "nsh/test";

test("shows PDF path input and page controls", async () => {
  const app = await open(".");
  expect(app.texts()).toContain("Enter a PDF path and press Open.");
  expect(app.texts()).toContain("Open PDF");
  expect(app.texts()).toContain("Previous");
  expect(app.texts()).toContain("Next");
});
