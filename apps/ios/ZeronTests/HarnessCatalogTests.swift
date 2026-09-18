import XCTest
@testable import Zeron

final class HarnessCatalogTests: XCTestCase {
    func testCodexFallbackStartsWithAstraAndExposesItsTraits() {
        let models = HarnessCatalog.models(for: "codex")
        let astra = models.first

        XCTAssertEqual(astra?.id, "gpt-6-astra")
        XCTAssertEqual(astra?.label, "GPT-6-Astra")
        XCTAssertEqual(astra?.reasoningLevels,
                       ["low", "medium", "high", "xhigh", "max", "ultra"])
        XCTAssertEqual(astra?.options.first?.id, "serviceTier")
        XCTAssertEqual(astra?.options.first?.choices.map(\.id), ["default", "fast"])
    }

    func testDefaultReasoningMatchesDesktopPreference() {
        let astra = HarnessCatalog.defaultModel(for: "codex")
        XCTAssertEqual(HarnessCatalog.defaultReasoning(for: astra), "high")

        let short = ModelInfo(id: "short", label: "Short", description: nil,
                              reasoningLevels: ["low", "medium"])
        XCTAssertEqual(HarnessCatalog.defaultReasoning(for: short), "medium")
    }

    func testChoiceFallsBackToAdvertisedDefault() {
        let option = HarnessCatalog.defaultModel(for: "codex").options[0]
        XCTAssertEqual(HarnessCatalog.selectedChoice(for: option, selectedId: "fast").label, "Fast")
        XCTAssertEqual(HarnessCatalog.selectedChoice(for: option, selectedId: "stale").id, "default")
    }

    func testAmpFallbackExposesDialModesAndEffortLadder() {
        XCTAssertEqual(HarnessCatalog.label(for: "amp"), "Amp")
        let models = HarnessCatalog.models(for: "amp")
        XCTAssertEqual(models.map(\.id), ["low", "medium", "high", "ultra"])
        XCTAssertEqual(models.first?.reasoningLevels,
                       ["minimal", "low", "medium", "high", "xhigh", "max"])
        XCTAssertEqual(BrandMark.forHarness("amp"), .amp)
    }

    func testAntigravityFallbackLabelsAndMark() {
        XCTAssertEqual(HarnessCatalog.label(for: "antigravity"), "Antigravity")
        let models = HarnessCatalog.models(for: "antigravity")
        XCTAssertEqual(models.first?.id, "gemini-3.8-flash-high")
        XCTAssertEqual(models.first?.reasoningLevels, ["low", "medium", "high"])
        XCTAssertEqual(BrandMark.forHarness("antigravity"), .antigravity)
    }

    func testKimiFallbackLabelsAndMark() {
        XCTAssertEqual(HarnessCatalog.label(for: "kimi-code"), "Kimi Code")
        let models = HarnessCatalog.models(for: "kimi-code")
        XCTAssertEqual(models.first?.id, "k3")
        XCTAssertEqual(models.first?.reasoningLevels, ["low", "high", "max"])
        XCTAssertEqual(BrandMark.forHarness("kimi-code"), .kimi)
    }

    func testWorkingFlavourLeadsWithCosmosing() {
        XCTAssertEqual(Motion.flavourWords.first, "Cosmosing")
    }
}
