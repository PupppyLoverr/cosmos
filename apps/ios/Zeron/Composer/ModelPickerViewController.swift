import UIKit

final class ModelPickerViewController: UIViewController, UITableViewDataSource, UITableViewDelegate, UIPopoverPresentationControllerDelegate {
    private enum Filter: Equatable {
        case all
        case favorites
        case harness(String)
    }

    private struct Section {
        let title: String
        let models: [ModelChoice]
    }

    private static let favoritesStorageKey = "modelFavorites"

    private let models: [ModelChoice]
    private let selected: (harness: String, model: String?)
    private let lockedHarness: String?
    private let onSelect: (ModelChoice) -> Void
    private var filter: Filter = .all
    private var favorites = Set(UserDefaults.standard.stringArray(forKey: "modelFavorites") ?? [])
    private var sections: [Section] = []
    private let titleLabel = UILabel()
    private let closeButton = UIButton(type: .system)
    private let search = UITextField()
    private let searchRow = UIView()
    private let searchDivider = UIView()
    private let railScroll = UIScrollView()
    private let railStack = UIStackView()
    private let railDivider = UIView()
    private let tableView = UITableView(frame: .zero, style: .plain)
    private let rootStack = UIStackView()
    private let contentStack = UIStackView()

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
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .popover
        if lockedHarness == nil {
            let hasFavorites = models.contains {
                favorites.contains(Self.favoriteKey($0))
            }
            filter = hasFavorites ? .favorites : .harness(selected.harness)
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.elevated

        titleLabel.text = "Models"
        titleLabel.textColor = Palette.text
        titleLabel.font = scaled(.sansSemibold, size: 17, style: .headline)
        titleLabel.adjustsFontForContentSizeCategory = true

        var closeConfiguration = UIButton.Configuration.plain()
        closeConfiguration.image = UIImage(
            systemName: "chevron.left",
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .medium)
        )
        closeConfiguration.baseForegroundColor = Palette.secondary
        closeConfiguration.contentInsets = .zero
        closeButton.configuration = closeConfiguration
        closeButton.accessibilityLabel = "Close model picker"
        closeButton.addAction(UIAction { [weak self] _ in self?.dismiss(animated: true) }, for: .touchUpInside)
        closeButton.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            closeButton.widthAnchor.constraint(equalToConstant: 28),
            closeButton.heightAnchor.constraint(equalToConstant: 28),
        ])

        let header = UIStackView(arrangedSubviews: [closeButton, titleLabel, UIView()])
        header.axis = .horizontal
        header.alignment = .center
        header.spacing = 8

        configureSearch()
        configureRail()
        configureTable()

        contentStack.axis = .vertical
        contentStack.alignment = .fill
        contentStack.spacing = 8
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.addArrangedSubview(header)
        contentStack.addArrangedSubview(searchRow)
        contentStack.addArrangedSubview(tableView)

        rootStack.axis = .horizontal
        rootStack.alignment = .fill
        rootStack.spacing = 8
        rootStack.translatesAutoresizingMaskIntoConstraints = false
        rootStack.addArrangedSubview(railScroll)
        rootStack.addArrangedSubview(railDivider)
        rootStack.addArrangedSubview(contentStack)
        view.addSubview(rootStack)
        NSLayoutConstraint.activate([
            rootStack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 6),
            rootStack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -8),
            rootStack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 10),
            rootStack.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -10),
            railScroll.widthAnchor.constraint(equalToConstant: 44),
            railDivider.widthAnchor.constraint(equalToConstant: 1),
            searchRow.heightAnchor.constraint(equalToConstant: 37),
        ])

        renderSections()
        updatePreferredContentSize()
        updatePreferredContentSize()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        scrollToSelectedModel()
    }

    func present(from anchor: UIView, in presenter: UIViewController) {
        let screenWidth = anchor.window?.windowScene?.screen.bounds.width ?? UIScreen.main.bounds.width
        preferredContentSize = CGSize(width: min(320, max(1, screenWidth - 32)), height: fittedHeight)
        if let presentation = popoverPresentationController {
            presentation.delegate = self
            presentation.sourceView = anchor
            presentation.sourceRect = anchor.bounds
            presentation.permittedArrowDirections = [.down, .up]
        }
        presenter.present(self, animated: true)
    }

    func adaptivePresentationStyle(
        for controller: UIPresentationController,
        traitCollection: UITraitCollection
    ) -> UIModalPresentationStyle {
        .none
    }

    private func configureSearch() {
        search.borderStyle = .none
        search.font = scaled(.sans, size: 14, style: .body)
        search.textColor = Palette.text
        search.tintColor = Palette.text
        search.placeholder = "Search models…"
        search.accessibilityLabel = "Search models"
        search.accessibilityIdentifier = "model-search"
        search.autocorrectionType = .no
        search.clearButtonMode = .whileEditing
        let icon = UIImageView(image: UIImage(
            systemName: "magnifyingglass",
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .medium)
        ))
        icon.tintColor = Palette.tertiary
        icon.contentMode = .center
        icon.translatesAutoresizingMaskIntoConstraints = false
        searchRow.translatesAutoresizingMaskIntoConstraints = false
        search.translatesAutoresizingMaskIntoConstraints = false
        searchRow.addSubview(icon)
        searchRow.addSubview(search)
        searchDivider.translatesAutoresizingMaskIntoConstraints = false
        searchDivider.backgroundColor = Palette.hairline
        searchRow.addSubview(searchDivider)
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: searchRow.leadingAnchor, constant: 2),
            icon.centerYAnchor.constraint(equalTo: searchRow.centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
            icon.heightAnchor.constraint(equalToConstant: 16),
            search.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
            search.trailingAnchor.constraint(equalTo: searchRow.trailingAnchor, constant: -2),
            search.topAnchor.constraint(equalTo: searchRow.topAnchor),
            search.bottomAnchor.constraint(equalTo: searchDivider.topAnchor),
            searchDivider.leadingAnchor.constraint(equalTo: searchRow.leadingAnchor),
            searchDivider.trailingAnchor.constraint(equalTo: searchRow.trailingAnchor),
            searchDivider.bottomAnchor.constraint(equalTo: searchRow.bottomAnchor),
            searchDivider.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale),
        ])
        search.addTarget(self, action: #selector(queryChanged), for: .editingChanged)
    }

    private func configureTable() {
        tableView.backgroundColor = .clear
        tableView.separatorStyle = .none
        tableView.sectionHeaderTopPadding = 6
        tableView.rowHeight = UITableView.automaticDimension
        tableView.estimatedRowHeight = 64
        tableView.contentInsetAdjustmentBehavior = .never
        tableView.register(ModelPickerCell.self, forCellReuseIdentifier: ModelPickerCell.reuseIdentifier)
        tableView.dataSource = self
        tableView.delegate = self
    }

    @objc private func queryChanged() {
        renderRail()
        renderSections()
        updatePreferredContentSize()
    }

    private var harnesses: [String] {
        Self.harnesses(in: models, lockedTo: lockedHarness)
    }

    private var fittedHeight: CGFloat {
        let rowsHeight = sections.reduce(CGFloat(0)) { total, section in
            total + 30 + section.models.reduce(CGFloat(0)) { height, model in
                height + ((model.description?.isEmpty == false) ? 56 : 48)
            }
        }
        let emptyHeight: CGFloat = sections.isEmpty ? 64 : 0
        return min(460, max(136, 10 + 30 + 8 + 37 + rowsHeight + emptyHeight + 10))
    }

    private var selectedModelID: String? {
        selected.model ?? models.first(where: { $0.harness == selected.harness })?.id
    }

    private func configureRail() {
        railScroll.showsVerticalScrollIndicator = false
        railScroll.translatesAutoresizingMaskIntoConstraints = false
        railStack.axis = .vertical
        railStack.alignment = .center
        railStack.spacing = 4
        railStack.translatesAutoresizingMaskIntoConstraints = false
        railScroll.addSubview(railStack)
        railDivider.backgroundColor = Palette.hairline
        railDivider.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            railStack.centerXAnchor.constraint(equalTo: railScroll.frameLayoutGuide.centerXAnchor),
            railStack.topAnchor.constraint(equalTo: railScroll.contentLayoutGuide.topAnchor, constant: 6),
            railStack.bottomAnchor.constraint(equalTo: railScroll.contentLayoutGuide.bottomAnchor, constant: -6),
            railStack.widthAnchor.constraint(lessThanOrEqualTo: railScroll.frameLayoutGuide.widthAnchor),
        ])
        renderRail()
    }

    private func renderRail() {
        railStack.arrangedSubviews.forEach {
            railStack.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        let searching = !(search.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let selectedFilter: Filter? = searching ? nil : filter
        railStack.addArrangedSubview(railButton(
            image: UIImage(systemName: "star.fill"),
            label: "Favorites",
            identifier: "model-rail-favorites",
            selected: selectedFilter == .favorites
        ) { [weak self] in
            self?.filter = .favorites
            self?.renderRail()
            self?.renderSections()
            self?.updatePreferredContentSize()
        })
        for harness in harnesses {
            railStack.addArrangedSubview(railButton(
                image: BrandMarks.image(for: harness, side: 16)?.withRenderingMode(.alwaysTemplate),
                label: harnessLabel(harness),
                identifier: "model-rail-\(harness)",
                selected: selectedFilter == .harness(harness)
            ) { [weak self] in
                self?.filter = .harness(harness)
                self?.renderRail()
                self?.renderSections()
                self?.updatePreferredContentSize()
            })
        }
        railScroll.isHidden = lockedHarness != nil || harnesses.count <= 1
        railDivider.isHidden = railScroll.isHidden
    }

    private func railButton(
        image: UIImage?,
        label: String,
        identifier: String,
        selected: Bool,
        action: @escaping () -> Void
    ) -> UIButton {
        var configuration = UIButton.Configuration.plain()
        configuration.image = image?.withRenderingMode(.alwaysTemplate)
        configuration.baseForegroundColor = selected ? Palette.text : Palette.secondary
        configuration.background.backgroundColor = selected ? Palette.controlFill : .clear
        configuration.background.cornerRadius = 8
        configuration.contentInsets = .zero
        configuration.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 15, weight: .medium)
        let button = UIButton(configuration: configuration)
        button.accessibilityIdentifier = identifier
        button.accessibilityLabel = label
        button.accessibilityTraits = .button
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: 32),
            button.heightAnchor.constraint(equalToConstant: 32),
        ])
        button.addAction(UIAction { _ in action() }, for: .touchUpInside)
        return button
    }

    private func renderSections(keepingOffset: Bool = false) {
        let contentOffset = tableView.contentOffset
        let query = (search.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let visible = models.filter { lockedHarness == nil || $0.harness == lockedHarness }
        if !query.isEmpty {
            sections = group(matching(visible, query: query))
        } else if lockedHarness != nil {
            let favoriteModels = visible.filter(isFavorite)
            sections = favoriteModels.isEmpty ? [] : [Section(title: "Favorites", models: favoriteModels)]
            sections.append(contentsOf: group(visible))
        } else {
            switch filter {
            case .all:
                sections = group(visible)
            case .favorites:
                let favoriteModels = visible.filter(isFavorite)
                sections = favoriteModels.isEmpty ? [] : [Section(title: "Favorites", models: favoriteModels)]
            case let .harness(harness):
                let harnessModels = visible.filter { $0.harness == harness }
                sections = harnessModels.isEmpty ? [] : [Section(title: harnessLabel(harness), models: harnessModels)]
            }
        }
        tableView.reloadData()
        tableView.backgroundView = emptyState(query: query)
        if keepingOffset {
            tableView.layoutIfNeeded()
            let maxOffset = max(0, tableView.contentSize.height - tableView.bounds.height)
            tableView.setContentOffset(CGPoint(x: contentOffset.x, y: min(contentOffset.y, maxOffset)), animated: false)
        }
    }

    private func updatePreferredContentSize() {
        guard preferredContentSize.width > 0 else { return }
        preferredContentSize.height = fittedHeight
    }

    private func matching(_ models: [ModelChoice], query: String) -> [ModelChoice] {
        guard !query.isEmpty else { return models }
        return models.filter { model in
            [model.label, model.id, model.description ?? "", model.harnessLabel]
                .contains { $0.localizedCaseInsensitiveContains(query) }
        }
    }

    private func emptyState(query: String) -> UILabel? {
        guard sections.isEmpty else { return nil }
        let label = UILabel()
        if !query.isEmpty {
            label.text = "No models match"
        } else if filter == .favorites {
            label.text = "No favorites yet. Tap the star on a model to add it."
        } else {
            label.text = "No models available"
        }
        label.font = scaled(.sans, size: 14, style: .body)
        label.textColor = Palette.secondary
        label.textAlignment = .center
        label.numberOfLines = 0
        return label
    }

    private func scrollToSelectedModel() {
        guard let selectedModelID else { return }
        for (sectionIndex, section) in sections.enumerated() {
            guard let rowIndex = section.models.firstIndex(where: {
                $0.harness == selected.harness && $0.id == selectedModelID
            }) else { continue }
            let indexPath = IndexPath(row: rowIndex, section: sectionIndex)
            tableView.layoutIfNeeded()
            let rowRect = tableView.rectForRow(at: indexPath)
            let insetTop = tableView.adjustedContentInset.top
            let insetBottom = tableView.adjustedContentInset.bottom
            let viewH = tableView.bounds.height - insetTop - insetBottom
            let minY = -insetTop
            let maxY = max(minY, tableView.contentSize.height - tableView.bounds.height + insetBottom)

            if abs(tableView.contentOffset.y - minY) < 1, rowRect.maxY <= viewH {
                tableView.setContentOffset(CGPoint(x: 0, y: minY), animated: false)
                return
            }

            var target = rowRect.midY - insetTop - viewH / 2
            target = min(max(target, minY), maxY)

            let visibleTop = target + insetTop
            let sectionAtTop = tableView.indexPathForRow(at: CGPoint(x: 1, y: visibleTop))?.section ?? indexPath.section
            let headerHeight = tableView.rectForHeader(inSection: sectionAtTop).height
            let probe = CGPoint(x: 1, y: visibleTop + headerHeight + 1)
            let p = tableView.indexPathForRow(at: probe) ?? indexPath
            let headerRect = tableView.rectForHeader(inSection: p.section)
            if p.row == 0 {
                target = headerRect.minY - insetTop
            } else {
                target = tableView.rectForRow(at: p).minY - headerRect.height - insetTop
            }
            target = max(target, minY)
            tableView.setContentOffset(CGPoint(x: 0, y: target), animated: false)
            return
        }
    }

    private func toggleFavorite(_ model: ModelChoice) {
        let key = Self.favoriteKey(model)
        if favorites.contains(key) {
            favorites.remove(key)
        } else {
            favorites.insert(key)
        }
        UserDefaults.standard.set(favorites.sorted(), forKey: Self.favoritesStorageKey)
        renderRail()
        renderSections(keepingOffset: true)
        updatePreferredContentSize()
    }

    private func isFavorite(_ model: ModelChoice) -> Bool {
        favorites.contains(Self.favoriteKey(model))
    }

    private func group(_ models: [ModelChoice]) -> [Section] {
        var order: [String] = []
        var byHarness: [String: [ModelChoice]] = [:]
        for model in models {
            if byHarness[model.harness] == nil { order.append(model.harness) }
            byHarness[model.harness, default: []].append(model)
        }
        return order.compactMap { harness in
            byHarness[harness].map { Section(title: harnessLabel(harness), models: $0) }
        }
    }

    private func harnessLabel(_ harness: String) -> String {
        models.first(where: { $0.harness == harness })?.harnessLabel ?? HarnessNames.label(harness)
    }

    private func scaled(_ font: FaceRole, size: CGFloat, style: UIFont.TextStyle) -> UIFont {
        UIFontMetrics(forTextStyle: style).scaledFont(for: Fonts.ui(font, size))
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

    func numberOfSections(in tableView: UITableView) -> Int {
        sections.count
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        sections[section].models.count
    }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let model = sections[indexPath.section].models[indexPath.row]
        let cell = tableView.dequeueReusableCell(
            withIdentifier: ModelPickerCell.reuseIdentifier,
            for: indexPath
        ) as! ModelPickerCell
        let selectedModel = model.harness == selected.harness && model.id == selectedModelID
        cell.configure(model: model, selected: selectedModel, favorite: isFavorite(model)) { [weak self] in
            self?.toggleFavorite(model)
        }
        return cell
    }

    func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        let model = sections[indexPath.section].models[indexPath.row]
        UISelectionFeedbackGenerator().selectionChanged()
        onSelect(model)
        dismiss(animated: true)
    }

    func tableView(_ tableView: UITableView, viewForHeaderInSection section: Int) -> UIView? {
        guard !shouldHideLockedHarnessHeader(in: section) else { return nil }
        let container = UIView()
        container.backgroundColor = Palette.elevated
        let label = UILabel()
        label.text = sections[section].title
        label.textColor = Palette.tertiary
        label.font = scaled(.sansMedium, size: 12, style: .caption1)
        label.adjustsFontForContentSizeCategory = true
        label.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 20),
            label.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -20),
            label.topAnchor.constraint(equalTo: container.topAnchor, constant: 8),
            label.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -2),
        ])
        return container
    }

    func tableView(_ tableView: UITableView, heightForHeaderInSection section: Int) -> CGFloat {
        shouldHideLockedHarnessHeader(in: section) ? 0 : 30
    }

    private func shouldHideLockedHarnessHeader(in section: Int) -> Bool {
        guard
            lockedHarness != nil,
            filter == .all,
            sections.count == 1,
            (search.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return false
        }
        return sections[section].title != "Favorites"
    }

    func tableView(_ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath) -> UISwipeActionsConfiguration? {
        let model = sections[indexPath.section].models[indexPath.row]
        let favorited = isFavorite(model)
        let action = UIContextualAction(
            style: .normal,
            title: favorited ? "Unfavorite" : "Favorite"
        ) { [weak self] _, _, completion in
            self?.toggleFavorite(model)
            completion(true)
        }
        action.image = UIImage(systemName: favorited ? "star.slash" : "star.fill")
        action.backgroundColor = favorited ? Palette.tertiary : Palette.text
        let configuration = UISwipeActionsConfiguration(actions: [action])
        configuration.performsFirstActionWithFullSwipe = false
        return configuration
    }

    func tableView(
        _ tableView: UITableView,
        contextMenuConfigurationForRowAt indexPath: IndexPath,
        point: CGPoint
    ) -> UIContextMenuConfiguration? {
        let model = sections[indexPath.section].models[indexPath.row]
        let favorited = isFavorite(model)
        let favorite = UIAction(
            title: favorited ? "Unfavorite" : "Favorite",
            image: UIImage(systemName: favorited ? "star.slash" : "star.fill")
        ) { [weak self] _ in
            self?.toggleFavorite(model)
        }
        let use = UIAction(title: "Use this model") { [weak self] _ in
            guard let self else { return }
            UISelectionFeedbackGenerator().selectionChanged()
            self.onSelect(model)
            self.dismiss(animated: true)
        }
        return UIContextMenuConfiguration(identifier: model.id as NSString, previewProvider: nil) { _ in
            UIMenu(children: [favorite, use])
        }
    }
}

private final class ModelPickerCell: UITableViewCell {
    static let reuseIdentifier = "ModelPickerCell"

    private let selectionBackground = UIView()
    private let mark = UIImageView()
    private let title = UILabel()
    private let detail = UILabel()
    private let favoriteButton = UIButton(type: .system)
    private let checkmark = UIImageView(image: UIImage(systemName: "checkmark"))
    private let labels = UIStackView()
    private let rowContent = UIStackView()
    private var minimumHeightConstraint: NSLayoutConstraint!
    private var selectedModel = false
    private var toggleFavorite: (() -> Void)?

    override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
        super.init(style: style, reuseIdentifier: reuseIdentifier)
        backgroundColor = .clear
        contentView.backgroundColor = .clear
        selectionBackground.layer.cornerRadius = 10
        selectionBackground.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(selectionBackground)
        minimumHeightConstraint = contentView.heightAnchor.constraint(greaterThanOrEqualToConstant: 48)
        NSLayoutConstraint.activate([
            selectionBackground.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 6),
            selectionBackground.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -6),
            selectionBackground.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 3),
            selectionBackground.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -3),
            minimumHeightConstraint,
        ])

        mark.contentMode = .scaleAspectFit
        mark.tintColor = Palette.text
        mark.setContentHuggingPriority(.required, for: .horizontal)
        title.font = dynamicFont(.sansMedium, size: 15, style: .body)
        title.textColor = Palette.text
        title.lineBreakMode = .byTruncatingTail
        title.numberOfLines = 1
        title.adjustsFontForContentSizeCategory = true
        detail.font = dynamicFont(.sans, size: 12.5, style: .caption1)
        detail.textColor = Palette.secondary
        detail.numberOfLines = 1
        detail.lineBreakMode = .byTruncatingTail
        detail.adjustsFontForContentSizeCategory = true
        labels.axis = .vertical
        labels.alignment = .fill
        labels.spacing = 2
        labels.addArrangedSubview(title)
        labels.addArrangedSubview(detail)
        labels.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        favoriteButton.contentHorizontalAlignment = .center
        favoriteButton.contentVerticalAlignment = .center
        favoriteButton.setContentHuggingPriority(.required, for: .horizontal)
        favoriteButton.addAction(UIAction { [weak self] _ in
            self?.toggleFavorite?()
        }, for: .touchUpInside)
        checkmark.tintColor = Palette.text
        checkmark.contentMode = .scaleAspectFit
        checkmark.setContentHuggingPriority(.required, for: .horizontal)
        rowContent.axis = .horizontal
        rowContent.alignment = .center
        rowContent.spacing = 9
        rowContent.translatesAutoresizingMaskIntoConstraints = false
        rowContent.addArrangedSubview(mark)
        rowContent.addArrangedSubview(labels)
        rowContent.addArrangedSubview(favoriteButton)
        rowContent.addArrangedSubview(checkmark)
        contentView.addSubview(rowContent)
        NSLayoutConstraint.activate([
            rowContent.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 16),
            rowContent.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -16),
            rowContent.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 5),
            rowContent.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -5),
            mark.widthAnchor.constraint(equalToConstant: 18),
            mark.heightAnchor.constraint(equalToConstant: 18),
            favoriteButton.widthAnchor.constraint(equalToConstant: 28),
            favoriteButton.heightAnchor.constraint(equalToConstant: 28),
            checkmark.widthAnchor.constraint(equalToConstant: 13),
            checkmark.heightAnchor.constraint(equalToConstant: 13),
        ])
        selectionStyle = .default
        isAccessibilityElement = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func setHighlighted(_ highlighted: Bool, animated: Bool) {
        super.setHighlighted(highlighted, animated: animated)
        updateSelectionBackground()
    }

    func configure(model: ModelChoice, selected: Bool, favorite: Bool, onToggleFavorite: @escaping () -> Void) {
        mark.image = BrandMarks.image(for: model.harness, side: 18)?.withRenderingMode(.alwaysTemplate)
        mark.isHidden = mark.image == nil
        title.text = model.label
        title.textColor = Palette.text
        title.font = dynamicFont(selected ? .sansSemibold : .sansMedium, size: 15, style: .body)
        detail.text = model.description
        detail.isHidden = model.description?.isEmpty ?? true
        minimumHeightConstraint.constant = detail.isHidden ? 48 : 56
        favoriteButton.setImage(
            UIImage(
                systemName: favorite ? "star.fill" : "star",
                withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .regular)
            ),
            for: .normal
        )
        favoriteButton.tintColor = favorite ? Palette.text : Palette.tertiary
        favoriteButton.accessibilityIdentifier = "model-favorite-\(model.id)"
        favoriteButton.accessibilityLabel = favorite ? "Unfavorite" : "Favorite"
        favoriteButton.accessibilityTraits = .button
        checkmark.isHidden = !selected
        selectedModel = selected
        toggleFavorite = onToggleFavorite
        accessibilityIdentifier = "model-row-\(model.id)"
        accessibilityLabel = [model.label, model.description]
            .compactMap { $0 }
            .filter { !$0.isEmpty }
            .joined(separator: ", ")
        accessibilityValue = selected ? "selected" : "not selected"
        accessibilityTraits = .button
        let actionName = favorite ? "Unfavorite" : "Favorite"
        accessibilityCustomActions = [
            UIAccessibilityCustomAction(name: actionName) { [weak self] _ in
                guard let self, let toggleFavorite = self.toggleFavorite else { return false }
                toggleFavorite()
                return true
            },
        ]
        updateSelectionBackground()
    }

    private func updateSelectionBackground() {
        selectionBackground.backgroundColor = isHighlighted
            ? Palette.controlFill
            : (selectedModel ? Palette.controlFill : .clear)
    }

    private func dynamicFont(_ face: FaceRole, size: CGFloat, style: UIFont.TextStyle) -> UIFont {
        UIFontMetrics(forTextStyle: style).scaledFont(for: Fonts.ui(face, size))
    }
}
