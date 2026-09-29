import { expect, test } from "@playwright/test";

test("workflow page is reachable on a phone and never silently creates an identity", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/workflows");
  await expect(
    page.getByRole("heading", { name: "Channel workflows" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Connect signer" }),
  ).toBeDisabled();
  await expect(
    page.getByText("No NIP-07 signer detected.", { exact: false }),
  ).toBeVisible();
  await expect(page.locator("body")).toHaveJSProperty("scrollWidth", 390);
  await page.screenshot({
    path: "screenshots/workflows-390.png",
    fullPage: true,
  });
});
