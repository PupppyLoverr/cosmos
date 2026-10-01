import UIKit

final class ModelPickerViewController: UIViewController, UIPopoverPresentationControllerDelegate {
    private enum RailSelection: Equatable {
        case favorites
        case harness(String)
    }

    private let models: [ModelChoice]
    private let selected: (harness: String, model: String?)
    private let lockedHarness: String?
    private let onSelect: (ModelChoice) -> Void
    private var railSelection: RailSelection
    private var favorites = Set(UserDefaults.standard.stringArray(forKey: "modelFavorites") ?? [])
    private let rail = UIStackView()
    private let content = UIStackView()
    private let results = UIStackView()
    private let resultScroll = UIScrollView()
    private let search = UITextField()

    init(
        models: [ModelChoice],
        selected: (harness: String, model: String?),
        lockedHarness: String? = nil,
        onSelect: @escaping (ModelChoice) -> Void
    ) {
        self.models = models
        self.selected = selected
        self.lockedHarness = lockedHarness
        self.onSelect = onSelect
        let harnesses = Self.harnesses(in: models, lockedTo: lockedHarness)
        let visibleFavorites = models.contains { model in
            (lockedHarness == nil || model.harness == lockedHarness)
                && UserDefaults.standard.stringArray(forKey: "modelFavorites")?.contains(Self.favoriteKey(model)) == true
        }
        if visibleFavorites {
            railSelection = .favorites
        } else if harnesses.contains(selected.harness) {
            railSelection = .harness(selected.harness)
        } else if let first = harnesses.first {
            railSelection = .harness(first)
        } else {
            railSelection = .favorites
        }
        super.init(nibName: nil, bundle: nil)
        preferredContentSize = CGSize(width: 320, height: 440)
        modalPresentationStyle = .popover
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        view.layer.shadowColor = UIColor.black.cgColor
        view.layer.shadowOpacity = 0.16
        view.layer.shadowRadius = 20
        view.layer.shadowOffset = CGSize(width: 0, height: 8)

        let surface = UIView()
        surface.translatesAutoresizingMaskIntoConstraints = false
        surface.backgroundColor = Palette.elevated
        surface.layer.cornerRadius = 16
        surface.layer.borderWidth = 1
        surface.layer.borderColor = Palette.hairline.cgColor
        view.addSubview(surface)

        rail.axis = .vertical
        rail.alignment = .center
        rail.spacing = 8
        let railScroll = UIScrollView()
        railScroll.translatesAutoresizingMaskIntoConstraints = false
        railScroll.showsVerticalScrollIndicator = false
        railScroll.addSubview(rail)
        rail.translatesAutoresizingMaskIntoConstraints = false

        let divider = UIView()
        divider.translatesAutoresizingMaskIntoConstraints = false
        divider.backgroundColor = Palette.hairline

        content.axis = .vertical
        content.alignment = .fill
        content.spacing = 11
        content.translatesAutoresizingMaskIntoConstraints = false
        surface.addSubview(railScroll)
        surface.addSubview(divider)
        surface.addSubview(content)

        let header = UIStackView()
        header.axis = .horizontal
        header.alignment = .center
        header.spacing = 7
        let back = UIButton(type: .system)
        back.setImage(UIImage(systemName: "chevron.left"), for: .normal)
        back.tintColor = Palette.secondary
        back.accessibilityLabel = "Back"
        back.addAction(UIAction { [weak self] _ in self?.dismiss(animated: true) }, for: .touchUpInside)
        let title = UILabel()
        title.text = "Models"
        title.textColor = Palette.text
        title.font = scaled(.sansSemibold, size: 16, style: .headline)
        header.addArrangedSubview(back)
        header.addArrangedSubview(title)

        search.borderStyle = .none
        search.font = scaled(.sans, size: 14, style: .body)
        search.textColor = Palette.text
        search.tintColor = Palette.accent
        search.placeholder = "Search models…"
        search.accessibilityLabel = "Search models"
        search.autocorrectionType = .no
        search.clearButtonMode = .whileEditing
        search.backgroundColor = Palette.controlFill
        search.layer.cornerRadius = 10
        search.leftView = UIImageView(image: UIImage(systemName: "magnifyingglass"))
        if let icon = search.leftView as? UIImageView {
            icon.tintColor = Palette.tertiary
            icon.contentMode = .center
            icon.frame = CGRect(x: 0, y: 0, width: 34, height: 36)
        }
        search.leftViewMode = .always
        search.heightAnchor.constraint(equalToConstant: 38).isActive = true
        search.addTarget(self, action: #selector(queryChanged), for: .editingChanged)

        results.axis = .vertical
        results.alignment = .fill
        results.spacing = 6
        resultScroll.translatesAutoresizingMaskIntoConstraints = false
        resultScroll.showsVerticalScrollIndicator = true
        resultScroll.addSubview(results)
        results.translatesAutoresizingMaskIntoConstraints = false

        content.addArrangedSubview(header)
        content.addArrangedSubview(search)
        content.addArrangedSubview(resultScroll)

        NSLayoutConstraint.activate([
            surface.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            surface.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            surface.topAnchor.constraint(equalTo: view.topAnchor),
            surface.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            railScroll.leadingAnchor.constraint(equalTo: surface.leadingAnchor, constant: 7),
            railScroll.topAnchor.constraint(equalTo: surface.topAnchor, constant: 12),
            railScroll.bottomAnchor.constraint(equalTo: surface.bottomAnchor, constant: -12),
            railScroll.widthAnchor.constraint(equalToConstant: 42),
            rail.leadingAnchor.constraint(equalTo: railScroll.contentLayoutGuide.leadingAnchor),
            rail.trailingAnchor.constraint(equalTo: railScroll.contentLayoutGuide.trailingAnchor),
            rail.topAnchor.constraint(equalTo: railScroll.contentLayoutGuide.topAnchor),
            rail.bottomAnchor.constraint(equalTo: railScroll.contentLayoutGuide.bottomAnchor),
            rail.widthAnchor.constraint(equalTo: railScroll.frameLayoutGuide.widthAnchor),
            divider.leadingAnchor.constraint(equalTo: railScroll.trailingAnchor, constant: 6),
            divider.topAnchor.constraint(equalTo: surface.topAnchor, constant: 12),
            divider.bottomAnchor.constraint(equalTo: surface.bottomAnchor, constant: -12),
            divider.widthAnchor.constraint(equalToConstant: 1),
            content.leadingAnchor.constraint(equalTo: divider.trailingAnchor, constant: 12),
            content.trailingAnchor.constraint(equalTo: surface.trailingAnchor, constant: -12),
            content.topAnchor.constraint(equalTo: surface.topAnchor, constant: 13),
            content.bottomAnchor.constraint(equalTo: surface.bottomAnchor, constant: -12),
            back.widthAnchor.constraint(equalToConstant: 27),
            back.heightAnchor.constraint(equalToConstant: 28),
            resultScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 0),
            results.leadingAnchor.constraint(equalTo: resultScroll.contentLayoutGuide.leadingAnchor),
            results.trailingAnchor.constraint(equalTo: resultScroll.contentLayoutGuide.trailingAnchor),
            results.topAnchor.constraint(equalTo: resultScroll.contentLayoutGuide.topAnchor),
            results.bottomAnchor.constraint(equalTo: resultScroll.contentLayoutGuide.bottomAnchor),
            results.widthAnchor.constraint(equalTo: resultScroll.frameLayoutGuide.widthAnchor),
        ])
        renderRail()
        renderResults()
        if let popover = popoverPresentationController {
            popover.delegate = self
            popover.backgroundColor = .clear
        }
    }

    func present(from anchor: UIView, in presenter: UIViewController) {
        modalPresentationStyle = .popover
        if let popover = popoverPresentationController {
            popover.delegate = self
            popover.sourceView = anchor
            popover.sourceRect = anchor.bounds
            popover.permittedArrowDirections = [.up, .down]
        }
        presenter.present(self, animated: true)
    }

    func adaptivePresentationStyle(for controller: UIPresentationController, traitCollection: UITraitCollection) -> UIModalPresentationStyle {
        .none
    }

    @objc private func queryChanged() {
        renderResults()
    }

    private var harnesses: [String] {
        Self.harnesses(in: models, lockedTo: lockedHarness)
    }

    private func renderRail() {
        rail.arrangedSubviews.forEach { $0.removeFromSuperview() }
        rail.addArrangedSubview(railButton(selection: .favorites, symbol: "star.fill", label: "Favorites"))
        for harness in harnesses {
            let button = railButton(selection: .harness(harness), symbol: nil, label: harnessLabel(harness))
            if let image = BrandMarks.image(for: harness, side: 17) {
                button.setImage(image, for: .normal)
            }
            rail.addArrangedSubview(button)
        }
    }

    private func railButton(selection: RailSelection, symbol: String?, label: String) -> UIButton {
        var config = UIButton.Configuration.plain()
        config.image = symbol.flatMap { UIImage(systemName: $0) }
        config.baseForegroundColor = railSelection == selection ? Palette.accent : Palette.secondary
        config.background.backgroundColor = railSelection == selection ? Palette.accentSoft : .clear
        config.background.cornerRadius = 10
        config.contentInsets = .zero
        let button = UIButton(configuration: config)
        button.translatesAutoresizingMaskIntoConstraints = false
        button.widthAnchor.constraint(equalToConstant: 34).isActive = true
        button.heightAnchor.constraint(equalToConstant: 34).isActive = true
        button.accessibilityLabel = label
        button.accessibilityTraits = .button
        button.addAction(UIAction { [weak self] _ in
            self?.railSelection = selection
            self?.renderRail()
            self?.renderResults()
        }, for: .touchUpInside)
        return button
    }

    private func renderResults() {
        results.arrangedSubviews.forEach { $0.removeFromSuperview() }
        let query = (search.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let grouped: [(String, [ModelChoice])]
        if !query.isEmpty {
            let matches = models.filter { model in
                (lockedHarness == nil || model.harness == lockedHarness)
                    && [model.label, model.id, model.description ?? "", model.harnessLabel]
                        .contains { $0.localizedCaseInsensitiveContains(query) }
            }
            grouped = group(matches)
        } else if railSelection == .favorites {
            let matches = models.filter { isFavorite($0) && (lockedHarness == nil || $0.harness == lockedHarness) }
            grouped = matches.isEmpty ? [] : [("Favorites", matches)]
        } else if case let .harness(harness) = railSelection {
            let matches = models.filter { $0.harness == harness }
            grouped = matches.isEmpty ? [] : [(harnessLabel(harness), matches)]
        } else {
            grouped = []
        }

        if grouped.isEmpty {
            let empty = UILabel()
            empty.text = query.isEmpty && railSelection == .favorites
                ? "Star a model to pin it here"
                : "No models found"
            empty.font = scaled(.sans, size: 13, style: .body)
            empty.textColor = Palette.secondary
            empty.textAlignment = .center
            empty.numberOfLines = 0
            empty.heightAnchor.constraint(equalTo: resultScroll.frameLayoutGuide.heightAnchor).isActive = true
            results.addArrangedSubview(empty)
            return
        }

        for (section, choices) in grouped {
            let heading = UILabel()
            heading.text = section.uppercased()
            heading.font = scaled(.sansMedium, size: 10.5, style: .caption1)
            heading.textColor = Palette.tertiary
            heading.accessibilityTraits = .header
            results.addArrangedSubview(heading)
            for model in choices {
                results.addArrangedSubview(modelRow(model))
            }
        }
    }

    private func modelRow(_ model: ModelChoice) -> UIView {
        let selectedModelID = selected.model ?? models.first { $0.harness == selected.harness }?.id
        let selectedModel = model.harness == selected.harness && model.id == selectedModelID
        let row = UIControl()
        row.translatesAutoresizingMaskIntoConstraints = false
        row.backgroundColor = selectedModel ? Palette.accentSoft : .clear
        row.layer.cornerRadius = 10
        row.accessibilityLabel = [model.label, model.description].compactMap { $0 }.joined(separator: ", ")
        row.accessibilityValue = selectedModel ? "selected" : "not selected"
        row.accessibilityTraits = .button
        row.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.onSelect(model)
            self.dismiss(animated: true)
        }, for: .touchUpInside)

        let mark = UIImageView(image: BrandMarks.image(for: model.harness, side: 16))
        mark.contentMode = .scaleAspectFit
        mark.setContentHuggingPriority(.required, for: .horizontal)

        let title = UILabel()
        title.text = model.label
        title.font = scaled(.sansMedium, size: 15, style: .body)
        title.textColor = Palette.text
        title.lineBreakMode = .byTruncatingTail
        title.adjustsFontForContentSizeCategory = true

        let labels = UIStackView()
        labels.axis = .vertical
        labels.alignment = .fill
        labels.spacing = 2
        labels.addArrangedSubview(title)
        if let description = model.description, !description.isEmpty {
            let subtitle = UILabel()
            subtitle.text = description
            subtitle.font = scaled(.sans, size: 12, style: .caption1)
            subtitle.textColor = Palette.secondary
            subtitle.lineBreakMode = .byTruncatingTail
            subtitle.adjustsFontForContentSizeCategory = true
            labels.addArrangedSubview(subtitle)
        }

        let favorite = UIButton(type: .system)
        favorite.tintColor = isFavorite(model) ? Palette.accent : Palette.tertiary
        favorite.setImage(UIImage(systemName: isFavorite(model) ? "star.fill" : "star"), for: .normal)
        favorite.accessibilityLabel = isFavorite(model) ? "Unfavorite" : "Favorite"
        favorite.accessibilityTraits = .button
        favorite.addAction(UIAction { [weak self] _ in
            self?.toggleFavorite(model)
        }, for: .touchUpInside)
        favorite.widthAnchor.constraint(equalToConstant: 30).isActive = true
        favorite.heightAnchor.constraint(equalToConstant: 36).isActive = true

        let check = UIImageView(image: UIImage(systemName: "checkmark"))
        check.tintColor = Palette.accent
        check.isHidden = !selectedModel
        check.setContentHuggingPriority(.required, for: .horizontal)
        let stack = UIStackView(arrangedSubviews: [mark, labels, check, favorite])
        stack.axis = .horizontal
        stack.alignment = .center
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 9),
            stack.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -5),
            stack.topAnchor.constraint(equalTo: row.topAnchor, constant: 5),
            stack.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -5),
            row.heightAnchor.constraint(greaterThanOrEqualToConstant: model.description == nil ? 44 : 54),
        ])
        return row
    }

    private func toggleFavorite(_ model: ModelChoice) {
        let key = Self.favoriteKey(model)
        if favorites.contains(key) {
            favorites.remove(key)
        } else {
            favorites.insert(key)
        }
        UserDefaults.standard.set(favorites.sorted(), forKey: "modelFavorites")
        renderRail()
        renderResults()
    }

    private func isFavorite(_ model: ModelChoice) -> Bool {
        favorites.contains(Self.favoriteKey(model))
    }

    private func group(_ models: [ModelChoice]) -> [(String, [ModelChoice])] {
        var order: [String] = []
        var byHarness: [String: [ModelChoice]] = [:]
        for model in models {
            if byHarness[model.harness] == nil { order.append(model.harness) }
            byHarness[model.harness, default: []].append(model)
        }
        return order.compactMap { harness in
            byHarness[harness].map { (harnessLabel(harness), $0) }
        }
    }

    private func harnessLabel(_ harness: String) -> String {
        models.first(where: { $0.harness == harness })?.harnessLabel ?? HarnessNames.label(harness)
    }

    private static func harnesses(in models: [ModelChoice], lockedTo harness: String?) -> [String] {
        var seen = Set<String>()
        return models.compactMap { model in
            guard (harness == nil || model.harness == harness), seen.insert(model.harness).inserted else { return nil }
            return model.harness
        }
    }

    private static func favoriteKey(_ model: ModelChoice) -> String {
        "\(model.harness)/\(model.id)"
    }

    private func scaled(_ font: FaceRole, size: CGFloat, style: UIFont.TextStyle) -> UIFont {
        UIFontMetrics(forTextStyle: style).scaledFont(for: Fonts.ui(font, size))
    }
}
